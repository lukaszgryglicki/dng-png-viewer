use crate::images::{self, LoadedImage};
use anyhow::{Context, Result, ensure};
use std::{
    collections::{HashMap, VecDeque},
    path::{Path, PathBuf},
    sync::{Arc, Condvar, Mutex, mpsc},
    thread::{self, JoinHandle},
};

type Loaded = std::result::Result<Arc<LoadedImage>, Arc<str>>;
type Completion = (u64, Loaded);

#[derive(Default)]
struct State {
    center: usize,
    generation: u64,
    cache: HashMap<usize, Loaded>,
    pending: VecDeque<usize>,
    stopped: bool,
}

struct Shared {
    state: Mutex<State>,
    changed: Condvar,
    radius: usize,
}

impl Shared {
    fn store(&self, index: usize, result: Loaded) -> Loaded {
        let mut state = self.state.lock().expect("image cache lock");
        if !state.stopped && state.center.abs_diff(index) <= self.radius {
            let result = state.cache.entry(index).or_insert(result).clone();
            self.changed.notify_all();
            result
        } else {
            result
        }
    }

    fn next_preload(&self) -> Option<usize> {
        let mut state = self.state.lock().expect("image cache lock");
        loop {
            if state.stopped {
                return None;
            }
            if state.cache.contains_key(&state.center) {
                while let Some(index) = state.pending.pop_front() {
                    if index != state.center && !state.cache.contains_key(&index) {
                        return Some(index);
                    }
                }
            }
            state = self.changed.wait(state).expect("image cache lock");
        }
    }
}

pub(crate) struct ImageLoader {
    shared: Arc<Shared>,
    count: usize,
    request_tx: mpsc::Sender<(u64, usize)>,
    result_tx: mpsc::Sender<Completion>,
    result_rx: mpsc::Receiver<Completion>,
    foreground: JoinHandle<()>,
    background: Option<JoinHandle<()>>,
}

impl ImageLoader {
    pub fn new(paths: Vec<PathBuf>, radius: usize) -> Result<Self> {
        Self::with_decoder(paths, radius, images::decode)
    }

    fn with_decoder<F>(paths: Vec<PathBuf>, radius: usize, decoder: F) -> Result<Self>
    where
        F: Fn(&Path) -> Result<LoadedImage> + Send + Sync + 'static,
    {
        ensure!(!paths.is_empty(), "cannot load an empty playlist");
        let count = paths.len();
        let shared = Arc::new(Shared {
            state: Mutex::new(State::default()),
            changed: Condvar::new(),
            radius,
        });
        let decode = Arc::new(move |index: usize| -> Loaded {
            decoder(&paths[index])
                .map(Arc::new)
                .map_err(|error| Arc::from(format!("{error:#}")))
        });
        let (request_tx, request_rx) = mpsc::channel::<(u64, usize)>();
        let (result_tx, result_rx) = mpsc::channel();
        let foreground = {
            let shared = shared.clone();
            let decode = decode.clone();
            let result_tx = result_tx.clone();
            thread::Builder::new()
                .name("image-loader".into())
                .spawn(move || {
                    while let Ok(mut request) = request_rx.recv() {
                        while let Ok(newer) = request_rx.try_recv() {
                            request = newer;
                        }
                        let (generation, index) = request;
                        let cached = {
                            let state = shared.state.lock().expect("image cache lock");
                            if state.stopped || state.generation != generation {
                                continue;
                            }
                            state.cache.get(&index).cloned()
                        };
                        let result = cached.unwrap_or_else(|| shared.store(index, decode(index)));
                        if result_tx.send((generation, result)).is_err() {
                            break;
                        }
                    }
                })
                .context("starting image loader")?
        };
        let mut loader = Self {
            shared: shared.clone(),
            count,
            request_tx,
            result_tx,
            result_rx,
            foreground,
            background: None,
        };
        if radius > 0 && count > 1 {
            let pool = rayon::ThreadPoolBuilder::new()
                .num_threads(1)
                .thread_name(|_| "preload-pixels".into())
                .build()
                .context("starting background pixel worker")?;
            loader.background = Some(
                thread::Builder::new()
                    .name("image-preloader".into())
                    .spawn(move || {
                        while let Some(index) = shared.next_preload() {
                            let result = pool.install(|| decode(index));
                            drop(shared.store(index, result));
                        }
                    })
                    .context("starting image preloader")?,
            );
        }
        Ok(loader)
    }

    pub fn request(&self, generation: u64, index: usize) -> Result<()> {
        ensure!(index < self.count, "image index is outside the playlist");
        let cached = {
            let mut state = self.shared.state.lock().expect("image cache lock");
            state.center = index;
            state.generation = generation;
            state
                .cache
                .retain(|&other, _| index.abs_diff(other) <= self.shared.radius);
            state.pending.clear();
            for distance in 1..=self.shared.radius.min(self.count - 1) {
                if let Some(next) = index
                    .checked_add(distance)
                    .filter(|&next| next < self.count)
                {
                    state.pending.push_back(next);
                }
                if let Some(previous) = index.checked_sub(distance) {
                    state.pending.push_back(previous);
                }
            }
            state.cache.get(&index).cloned()
        };
        self.shared.changed.notify_all();
        if let Some(cached) = cached {
            self.result_tx.send((generation, cached))?;
        } else {
            self.request_tx
                .send((generation, index))
                .context("requesting image")?;
        }
        Ok(())
    }

    pub fn try_recv(&self) -> Result<Option<Completion>> {
        match self.result_rx.try_recv() {
            Ok(result) => Ok(Some(result)),
            Err(mpsc::TryRecvError::Empty) => {
                ensure!(
                    !self.foreground.is_finished(),
                    "image loader stopped unexpectedly"
                );
                ensure!(
                    !self
                        .background
                        .as_ref()
                        .is_some_and(JoinHandle::is_finished),
                    "image preloader stopped unexpectedly"
                );
                Ok(None)
            }
            Err(mpsc::TryRecvError::Disconnected) => anyhow::bail!("image loader disconnected"),
        }
    }
}

impl Drop for ImageLoader {
    fn drop(&mut self) {
        if let Ok(mut state) = self.shared.state.lock() {
            state.stopped = true;
            state.pending.clear();
            state.cache.clear();
        }
        self.shared.changed.notify_all();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use image::{DynamicImage, ImageBuffer, Luma};
    use std::{
        sync::atomic::{AtomicUsize, Ordering},
        time::{Duration, Instant},
    };

    fn paths(count: usize) -> Vec<PathBuf> {
        (0..count)
            .map(|index| PathBuf::from(index.to_string()))
            .collect()
    }

    fn index(path: &Path) -> usize {
        path.to_str().unwrap().parse().unwrap()
    }

    fn image(index: usize) -> Result<LoadedImage> {
        LoadedImage::new(DynamicImage::ImageLuma8(ImageBuffer::from_pixel(
            1,
            1,
            Luma([index as u8]),
        )))
    }

    fn loader(count: usize, radius: usize) -> (ImageLoader, Arc<Vec<AtomicUsize>>) {
        let calls = Arc::new((0..count).map(|_| AtomicUsize::new(0)).collect::<Vec<_>>());
        let recorded = calls.clone();
        let loader = ImageLoader::with_decoder(paths(count), radius, move |path| {
            let index = index(path);
            recorded[index].fetch_add(1, Ordering::SeqCst);
            if thread::current().name() == Some("preload-pixels") {
                assert_eq!(rayon::current_num_threads(), 1);
            }
            image(index)
        })
        .unwrap();
        (loader, calls)
    }

    fn receive(loader: &ImageLoader, generation: u64) -> Loaded {
        let (completed, image) = loader
            .result_rx
            .recv_timeout(Duration::from_secs(5))
            .unwrap();
        assert_eq!(completed, generation);
        image
    }

    fn cached(loader: &ImageLoader, expected: &[usize]) {
        let state = loader.shared.state.lock().unwrap();
        let (state, _) = loader
            .shared
            .changed
            .wait_timeout_while(state, Duration::from_secs(5), |state| {
                expected
                    .iter()
                    .any(|index| !state.cache.contains_key(index))
            })
            .unwrap();
        let mut actual: Vec<_> = state.cache.keys().copied().collect();
        actual.sort_unstable();
        assert_eq!(actual, expected);
    }

    #[test]
    fn radius_three_slides_reuses_both_sides_and_releases_evicted_images() {
        let (loader, calls) = loader(12, 3);
        loader.request(0, 4).unwrap();
        let original = receive(&loader, 0).unwrap();
        cached(&loader, &[1, 2, 3, 4, 5, 6, 7]);
        let evicted = {
            let state = loader.shared.state.lock().unwrap();
            Arc::downgrade(state.cache[&1].as_ref().unwrap())
        };
        loader.request(1, 5).unwrap();
        receive(&loader, 1).unwrap();
        cached(&loader, &[2, 3, 4, 5, 6, 7, 8]);
        assert!(evicted.upgrade().is_none());
        for index in 1..=8 {
            assert_eq!(calls[index].load(Ordering::SeqCst), 1, "index {index}");
        }
        assert_eq!(calls[0].load(Ordering::SeqCst), 0);
        assert_eq!(calls[9].load(Ordering::SeqCst), 0);
        loader.request(2, 4).unwrap();
        assert!(Arc::ptr_eq(&original, &receive(&loader, 2).unwrap()));
        cached(&loader, &[1, 2, 3, 4, 5, 6, 7]);
        assert_eq!(calls[1].load(Ordering::SeqCst), 2);
        for index in 2..=8 {
            assert_eq!(calls[index].load(Ordering::SeqCst), 1);
        }
    }

    #[test]
    fn preload_zero_keeps_only_current_image_and_starts_no_background_thread() {
        let (loader, calls) = loader(4, 0);
        assert!(loader.background.is_none());
        for (generation, index) in [(0, 0), (1, 1), (2, 0)] {
            loader.request(generation, index).unwrap();
            receive(&loader, generation).unwrap();
            cached(&loader, &[index]);
        }
        assert_eq!(calls[0].load(Ordering::SeqCst), 2);
        assert_eq!(calls[1].load(Ordering::SeqCst), 1);
        assert_eq!(calls[2].load(Ordering::SeqCst), 0);
        assert_eq!(calls[3].load(Ordering::SeqCst), 0);
    }

    #[test]
    fn edges_and_very_large_radii_are_bounded_by_the_playlist() {
        let (loader, _) = loader(8, 3);
        for (generation, index, expected) in [(0, 0, vec![0, 1, 2, 3]), (1, 7, vec![4, 5, 6, 7])] {
            loader.request(generation, index).unwrap();
            receive(&loader, generation).unwrap();
            cached(&loader, &expected);
        }
        let (wide, _) = self::loader(3, usize::MAX);
        wide.request(0, 0).unwrap();
        receive(&wide, 0).unwrap();
        cached(&wide, &[0, 1, 2]);
        let (single, _) = self::loader(1, usize::MAX);
        assert!(single.background.is_none());
        single.request(0, 0).unwrap();
        receive(&single, 0).unwrap();
        cached(&single, &[0]);
        assert!(single.request(1, 1).is_err());
        assert!(ImageLoader::new(Vec::new(), 3).is_err());
    }

    #[test]
    fn million_path_shuffle_keeps_prefetch_in_final_order_and_decoding_bounded() {
        let mut paths = paths(1_000_000);
        fastrand::Rng::with_seed(9123).shuffle(&mut paths);
        let plan = [
            vec![
                500_000, 500_001, 499_999, 500_002, 499_998, 500_003, 499_997,
            ],
            vec![999_999, 999_998, 999_997, 999_996],
            vec![0, 1, 2, 3],
        ]
        .map(|order| {
            let expected = order
                .iter()
                .map(|&position| index(&paths[position]))
                .collect::<Vec<_>>();
            let center = order[0];
            let mut cache = order;
            cache.sort_unstable();
            (center, cache, expected)
        });
        let calls = Arc::new(Mutex::new(Vec::new()));
        let recorded = calls.clone();
        let loader = ImageLoader::with_decoder(paths, 3, move |path| {
            let value = index(path);
            recorded.lock().unwrap().push(value);
            image(value)
        })
        .unwrap();
        assert_eq!(loader.count, 1_000_000);
        assert!(calls.lock().unwrap().is_empty());
        let mut total = 0;
        for (generation, (center, cache, expected)) in plan.into_iter().enumerate() {
            loader.request(generation as u64, center).unwrap();
            let loaded = receive(&loader, generation as u64).unwrap();
            assert_eq!(
                loaded.image.as_rgba8().unwrap().get_pixel(0, 0).0,
                [expected[0] as u8, expected[0] as u8, expected[0] as u8, 255]
            );
            cached(&loader, &cache);
            let mut actual = calls.lock().unwrap();
            assert_eq!(*actual, expected);
            total += actual.len();
            actual.clear();
        }
        assert_eq!(total, 15);
    }

    #[test]
    fn blocked_preload_cannot_block_current_decode_and_stale_results_are_discarded() {
        let (started_tx, started_rx) = mpsc::channel();
        let (release_tx, release_rx) = mpsc::channel();
        let release_rx = Mutex::new(release_rx);
        let loader = ImageLoader::with_decoder(paths(8), 1, move |path| {
            let index = index(path);
            if index == 1 {
                started_tx
                    .send(thread::current().name().unwrap().to_owned())
                    .unwrap();
                release_rx
                    .lock()
                    .unwrap()
                    .recv_timeout(Duration::from_secs(5))
                    .unwrap();
            }
            image(index)
        })
        .unwrap();
        loader.request(0, 0).unwrap();
        receive(&loader, 0).unwrap();
        assert_eq!(
            started_rx.recv_timeout(Duration::from_secs(5)).unwrap(),
            "preload-pixels"
        );
        loader.request(1, 6).unwrap();
        let current = receive(&loader, 1).unwrap();
        assert_eq!(
            current.image.as_rgba8().unwrap().get_pixel(0, 0).0,
            [6, 6, 6, 255]
        );
        release_tx.send(()).unwrap();
        cached(&loader, &[5, 6, 7]);
    }

    #[test]
    fn foreground_requests_coalesce_during_rapid_navigation() {
        let (started_tx, started_rx) = mpsc::channel();
        let (release_tx, release_rx) = mpsc::channel();
        let release_rx = Mutex::new(release_rx);
        let calls = Arc::new(Mutex::new(Vec::new()));
        let recorded = calls.clone();
        let loader = ImageLoader::with_decoder(paths(5), 0, move |path| {
            let index = index(path);
            recorded.lock().unwrap().push(index);
            if index == 0 {
                started_tx.send(()).unwrap();
                release_rx
                    .lock()
                    .unwrap()
                    .recv_timeout(Duration::from_secs(5))
                    .unwrap();
            }
            image(index)
        })
        .unwrap();
        loader.request(0, 0).unwrap();
        started_rx.recv_timeout(Duration::from_secs(5)).unwrap();
        for index in 1..=4 {
            loader.request(index as u64, index).unwrap();
        }
        release_tx.send(()).unwrap();
        receive(&loader, 0).unwrap();
        receive(&loader, 4).unwrap();
        cached(&loader, &[4]);
        assert_eq!(*calls.lock().unwrap(), vec![0, 4]);
    }

    #[test]
    fn cached_image_is_available_even_while_an_old_foreground_request_is_blocked() {
        let (started_tx, started_rx) = mpsc::channel();
        let (release_tx, release_rx) = mpsc::channel();
        let release_rx = Mutex::new(release_rx);
        let loader = ImageLoader::with_decoder(paths(8), 2, move |path| {
            let index = index(path);
            if index == 5 && thread::current().name() == Some("image-loader") {
                started_tx.send(()).unwrap();
                release_rx
                    .lock()
                    .unwrap()
                    .recv_timeout(Duration::from_secs(5))
                    .unwrap();
            }
            image(index)
        })
        .unwrap();
        loader.request(0, 2).unwrap();
        receive(&loader, 0).unwrap();
        cached(&loader, &[0, 1, 2, 3, 4]);
        loader.request(1, 5).unwrap();
        started_rx.recv_timeout(Duration::from_secs(5)).unwrap();
        loader.request(2, 4).unwrap();
        let current = receive(&loader, 2).unwrap();
        assert_eq!(
            current.image.as_rgba8().unwrap().get_pixel(0, 0).0,
            [4, 4, 4, 255]
        );
        release_tx.send(()).unwrap();
        receive(&loader, 1).unwrap();
        cached(&loader, &[2, 3, 4, 5, 6]);
    }

    #[test]
    fn preload_errors_are_cached_and_reported_only_when_selected() {
        let loader = ImageLoader::with_decoder(paths(2), 1, |path| {
            if index(path) == 1 {
                anyhow::bail!("broken neighbor");
            }
            image(0)
        })
        .unwrap();
        loader.request(0, 0).unwrap();
        receive(&loader, 0).unwrap();
        cached(&loader, &[0, 1]);
        assert!(loader.try_recv().unwrap().is_none());
        loader.request(1, 1).unwrap();
        assert_eq!(&*receive(&loader, 1).unwrap_err(), "broken neighbor");
        loader.request(2, 0).unwrap();
        receive(&loader, 2).unwrap();
    }

    #[test]
    fn shutdown_does_not_wait_for_an_in_progress_preload() {
        let (started_tx, started_rx) = mpsc::channel();
        let (release_tx, release_rx) = mpsc::channel();
        let release_rx = Mutex::new(release_rx);
        let loader = ImageLoader::with_decoder(paths(2), 1, move |path| {
            if index(path) == 1 {
                started_tx.send(()).unwrap();
                release_rx
                    .lock()
                    .unwrap()
                    .recv_timeout(Duration::from_secs(5))
                    .unwrap();
            }
            image(index(path))
        })
        .unwrap();
        loader.request(0, 0).unwrap();
        receive(&loader, 0).unwrap();
        started_rx.recv_timeout(Duration::from_secs(5)).unwrap();
        let start = Instant::now();
        drop(loader);
        assert!(start.elapsed() < Duration::from_millis(500));
        release_tx.send(()).unwrap();
    }
}
