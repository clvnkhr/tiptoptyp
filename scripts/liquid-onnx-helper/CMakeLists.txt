cmake_minimum_required(VERSION 3.18)
project(liquid_onnx_helper LANGUAGES CXX)

option(LIQUID_BUILD_HELPER "Build the native inference executable" ON)
include(CTest)

if(LIQUID_BUILD_HELPER)
  find_path(ONNXRUNTIME_INCLUDE_DIR onnxruntime_cxx_api.h
    HINTS "${ONNXRUNTIME_ROOT}/include" NO_DEFAULT_PATH REQUIRED)
  find_library(ONNXRUNTIME_LIBRARY NAMES onnxruntime
    HINTS "${ONNXRUNTIME_ROOT}/lib" NO_DEFAULT_PATH REQUIRED)
  add_executable(liquid-onnx main.cpp)
  target_compile_features(liquid-onnx PRIVATE cxx_std_20)
  target_include_directories(liquid-onnx SYSTEM PRIVATE "${ONNXRUNTIME_INCLUDE_DIR}")
  target_link_libraries(liquid-onnx PRIVATE "${ONNXRUNTIME_LIBRARY}")
  target_compile_options(liquid-onnx PRIVATE -Wall -Wextra -Werror)
  set_target_properties(liquid-onnx PROPERTIES
    BUILD_WITH_INSTALL_RPATH TRUE
    INSTALL_RPATH "@loader_path"
    INSTALL_RPATH_USE_LINK_PATH FALSE)
  if(CMAKE_BUILD_TYPE STREQUAL "Release")
    target_compile_options(liquid-onnx PRIVATE -O3 -DNDEBUG)
  endif()
endif()

if(BUILD_TESTING)
  add_executable(liquid-onnx-protocol-tests protocol_tests.cpp)
  target_compile_features(liquid-onnx-protocol-tests PRIVATE cxx_std_20)
  target_compile_options(liquid-onnx-protocol-tests PRIVATE -Wall -Wextra -Werror)
  add_test(NAME liquid-onnx-protocol COMMAND liquid-onnx-protocol-tests)
endif()
