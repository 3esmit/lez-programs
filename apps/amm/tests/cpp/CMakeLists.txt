cmake_minimum_required(VERSION 3.21)

if(CMAKE_SOURCE_DIR STREQUAL CMAKE_CURRENT_SOURCE_DIR)
    project(AmmRegistryTests LANGUAGES CXX)
    set(CMAKE_CXX_STANDARD 17)
    set(CMAKE_CXX_STANDARD_REQUIRED ON)
    find_package(Qt6 6.8 REQUIRED COMPONENTS Core Network Test)
    qt_standard_project_setup(REQUIRES 6.8)
    include(CTest)
else()
    find_package(Qt6 6.8 REQUIRED COMPONENTS Test)
endif()

function(amm_registry_test name source)
    qt_add_executable(${name}_test
        ${source}
        ../../src/RegistryLoader.cpp
        ../../src/RegistryLoader.h
    )
    target_compile_features(${name}_test PRIVATE cxx_std_17)
    target_include_directories(${name}_test PRIVATE ../../src)
    target_link_libraries(${name}_test PRIVATE
        Qt6::Core
        Qt6::Network
        Qt6::Test
    )
    add_test(NAME ${name} COMMAND ${name}_test)
endfunction()

amm_registry_test(amm_registry_loader RegistryLoaderTest.cpp)
amm_registry_test(amm_registry_status RegistryStatusTest.cpp)
