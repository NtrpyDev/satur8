cmake_minimum_required(VERSION 3.20)
project(satur8-kwin-effect LANGUAGES CXX)

# The KWin effect ABI is NOT stable: this must be built against the exact KWin
# version it will run under.
#
# Upstream KWin/KF6 ship cmake configs that hard-require extra-cmake-modules
# (ECM). When ECM isn't installed (e.g. a gaming box with the runtime but not
# the full KDE SDK) those find_package() configs fail even though every header
# and library is present. So we wire KWin and the few KF6 headers we touch
# directly. A distro package can replace this with the normal find_package path.
set(CMAKE_CXX_STANDARD 20)
set(CMAKE_CXX_STANDARD_REQUIRED ON)
set(CMAKE_AUTOMOC ON)
set(CMAKE_POSITION_INDEPENDENT_CODE ON)

find_package(Qt6 REQUIRED COMPONENTS Core Gui DBus Qml)  # Qml: KWin 6.8 headers include qqmlregistration.h

# --- KWin (headers + libkwin) -------------------------------------------------
set(KWIN_INCLUDE_DIR "/usr/include/kwin" CACHE PATH "KWin effect headers")
find_library(KWIN_LIBRARY NAMES kwin PATHS /usr/lib /usr/lib64 REQUIRED)

# The effect API changes between KWin minor versions (6.8 made drawWindow return
# bool), so the source branches on the version of the headers it is built with.
file(STRINGS "${KWIN_INCLUDE_DIR}/config-kwin.h" KWIN_VERSION_LINE
     REGEX "#define KWIN_PLUGIN_VERSION_STRING")
if(NOT KWIN_VERSION_LINE MATCHES "\"([0-9]+)\\.([0-9]+)")
    message(FATAL_ERROR "cannot read KWIN_PLUGIN_VERSION_STRING from ${KWIN_INCLUDE_DIR}/config-kwin.h")
endif()
set(SATUR8_KWIN_MAJOR ${CMAKE_MATCH_1})
set(SATUR8_KWIN_MINOR ${CMAKE_MATCH_2})
message(STATUS "Building against KWin ${SATUR8_KWIN_MAJOR}.${SATUR8_KWIN_MINOR}")

# --- KF6 headers we include transitively (KPluginFactory, KSharedConfig, ...) -
file(GLOB KF6_INCLUDE_CANDIDATES LIST_DIRECTORIES true "/usr/include/KF6/*")
set(KF6_INCLUDE_DIRS)
foreach(candidate IN LISTS KF6_INCLUDE_CANDIDATES)
    if(IS_DIRECTORY "${candidate}")
        list(APPEND KF6_INCLUDE_DIRS "${candidate}")
    endif()
endforeach()
find_library(KF6_COREADDONS_LIBRARY NAMES KF6CoreAddons PATHS /usr/lib /usr/lib64 REQUIRED)
find_library(KF6_CONFIGCORE_LIBRARY NAMES KF6ConfigCore PATHS /usr/lib /usr/lib64 REQUIRED)

add_library(satur8_effect MODULE
    satur8_effect.cpp
    satur8_effect.h
)

# KWin derives the effect id from the plugin's filename *including* any "lib"
# prefix. Emit plain "satur8.so" so the id is simply "satur8".
set_target_properties(satur8_effect PROPERTIES
    PREFIX ""
    OUTPUT_NAME "satur8")

target_include_directories(satur8_effect PRIVATE
    ${KWIN_INCLUDE_DIR}
    /usr/include/KF6
    ${KF6_INCLUDE_DIRS}
)

target_compile_definitions(satur8_effect PRIVATE
    SATUR8_KWIN_MAJOR=${SATUR8_KWIN_MAJOR}
    SATUR8_KWIN_MINOR=${SATUR8_KWIN_MINOR}
)

target_link_libraries(satur8_effect
    Qt6::Core
    Qt6::Gui
    Qt6::DBus
    Qt6::Qml
    ${KWIN_LIBRARY}
    ${KF6_COREADDONS_LIBRARY}
    ${KF6_CONFIGCORE_LIBRARY}
)

# KWin discovers binary effects as Qt plugins under <plugin path>/kwin/effects/plugins.
# Default to a user-writable location so no root is needed for development; a
# distro package would override this to the system Qt plugin dir.
if(NOT VIBRANCE_PLUGIN_INSTALL_DIR)
    set(VIBRANCE_PLUGIN_INSTALL_DIR
        "$ENV{HOME}/.local/lib/qt6/plugins/kwin/effects/plugins")
endif()

install(TARGETS satur8_effect
    LIBRARY DESTINATION "${VIBRANCE_PLUGIN_INSTALL_DIR}")
