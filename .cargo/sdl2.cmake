set(CMAKE_C_STANDARD 99 CACHE STRING "" FORCE)

# Unused controller code in bundled SDL2 conflicts with FreeBSD's libusb headers.
set(SDL_HIDAPI OFF CACHE BOOL "" FORCE)
set(SDL_JOYSTICK OFF CACHE BOOL "" FORCE)
set(SDL_HAPTIC OFF CACHE BOOL "" FORCE)

if(CMAKE_HOST_SYSTEM_NAME STREQUAL "FreeBSD")
    # SDL probes evdev before adding the FreeBSD ports include directory.
    list(APPEND CMAKE_REQUIRED_INCLUDES /usr/local/include)
endif()

if(CMAKE_HOST_SYSTEM_NAME STREQUAL "Linux")
    add_compile_options(-fcommon)
endif()
