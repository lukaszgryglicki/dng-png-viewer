if(CMAKE_VERSION VERSION_LESS 3.22)
    message(FATAL_ERROR "The viewer requires CMake 3.22 or newer for mandatory HEIF decoder checks")
endif()

set(CMAKE_REQUIRE_FIND_PACKAGE_LIBDE265 TRUE)
foreach(codec LIBDE265 AOM_DECODER)
    set(WITH_${codec} ON CACHE BOOL "" FORCE)
    set(WITH_${codec}_PLUGIN OFF CACHE BOOL "" FORCE)
endforeach()

# AOM's finder tries an optional CMake config before its pkg-config fallback.
get_property(dng_try_compile GLOBAL PROPERTY IN_TRY_COMPILE)
if(NOT dng_try_compile)
    function(dng_require_aom_decoder)
        if(NOT AOM_DECODER_FOUND)
            message(FATAL_ERROR "The viewer requires the libaom development package for AV1 decoding")
        endif()
    endfunction()
    cmake_language(DEFER CALL dng_require_aom_decoder)
endif()

# Keep HEVC/AV1 only; unused OpenH264 also has broken FreeBSD C++ linker metadata.
foreach(codec
    X264 OpenH264_DECODER RAV1E SvtEnc KVAZAAR
    JPEG_DECODER JPEG_ENCODER OpenJPEG_DECODER OpenJPEG_ENCODER
    OPENJPH_ENCODER OPEN_JPH_ENCODER FFMPEG_DECODER UVG266 VVDEC VVENC
)
    set(WITH_${codec} OFF CACHE BOOL "" FORCE)
endforeach()
