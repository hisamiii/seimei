# The app's echo canceller (echo.cc), linked with webrtc's audio processing that webrtc.sh builds once per ABI.
cmake_minimum_required(VERSION 3.22)
project(seimeiecho CXX)

set(WEBRTC ${CMAKE_CURRENT_SOURCE_DIR}/../../../build/webrtc/${ANDROID_ABI})
if(NOT EXISTS ${WEBRTC}/lib/libwebrtc-audio-processing-2.a)
    execute_process(COMMAND sh ${CMAKE_CURRENT_SOURCE_DIR}/webrtc.sh ${ANDROID_ABI} ${ANDROID_NDK} ${WEBRTC} RESULT_VARIABLE rc)
    if(NOT rc EQUAL 0)
        message(FATAL_ERROR "webrtc.sh failed for ${ANDROID_ABI}")
    endif()
endif()

add_library(seimeiecho SHARED echo.cc jni.cc)
set_target_properties(seimeiecho PROPERTIES CXX_STANDARD 17)
target_include_directories(seimeiecho PRIVATE ${WEBRTC}/include/webrtc-audio-processing-2 ${WEBRTC}/include)
target_compile_definitions(seimeiecho PRIVATE WEBRTC_POSIX WEBRTC_LIBRARY_IMPL)
file(GLOB_RECURSE ABSL ${WEBRTC}/../build-${ANDROID_ABI}/subprojects/libabsl_*.a)
# Only the JNI functions leave the library, and without debug info: the app ships it as is.
set_target_properties(seimeiecho PROPERTIES CXX_VISIBILITY_PRESET hidden)
target_link_options(seimeiecho PRIVATE -Wl,--exclude-libs,ALL -Wl,--gc-sections -Wl,--strip-all)
target_link_libraries(seimeiecho PRIVATE -Wl,--start-group ${WEBRTC}/lib/libwebrtc-audio-processing-2.a ${ABSL} -Wl,--end-group log)
