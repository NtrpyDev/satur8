cmake_minimum_required(VERSION 3.20)
project(vibrance-kwin-effect LANGUAGES CXX)

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

find_package(Qt6 REQUIRED COMPONENTS Core Gui DBus)

# --- KWin (headers + libkwin) -------------------------------------------------
set(KWIN_INCLUDE_DIR "/usr/include/kwin" CACHE PATH "KWin effect headers")
find_library(KWIN_LIBRARY NAMES kwin PATHS /usr/lib /usr/lib64 REQUIRED)

# --- KF6 headers we include transitively (KPluginFactory, KSharedConfig, ...) -
file(GLOB KF6_INCLUDE_DIRS "/usr/include/KF6/*")
find_library(KF6_COREADDONS_LIBRARY NAMES KF6CoreAddons PATHS /usr/lib /usr/lib64 REQUIRED)
find_library(KF6_CONFIGCORE_LIBRARY NAMES KF6ConfigCore PATHS /usr/lib /usr/lib64 REQUIRED)

add_library(vibrance_effect MODULE
    vibrance_effect.cpp
    vibrance_effect.h
)

target_include_directories(vibrance_effect PRIVATE
    ${KWIN_INCLUDE_DIR}
    /usr/include/KF6
    ${KF6_INCLUDE_DIRS}
)

target_link_libraries(vibrance_effect
    Qt6::Core
    Qt6::Gui
    Qt6::DBus
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

install(TARGETS vibrance_effect
    LIBRARY DESTINATION "${VIBRANCE_PLUGIN_INSTALL_DIR}")
