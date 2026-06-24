# RPM spec for Satur8. This mirrors the Arch PKGBUILD: build the Rust workspace
# and the C++ KWin effect from the release source archive, then install the same
# components into the system paths Fedora uses.
#
# It is written for COPR, where the build host has network access so Cargo can
# fetch crates during %build. A submission to official Fedora (Koji) would need
# the crates vendored instead, because Koji builds offline.

Name:           satur8
Version:        0.2.1
Release:        1%{?dist}
Summary:        Per-game digital vibrance for Linux

License:        GPL-3.0-or-later
URL:            https://github.com/NtrpyDev/satur8
Source0:        %{url}/releases/download/v%{version}/%{name}-v%{version}-source.tar.gz

# x86_64 is the only target the project ships and tests today.
ExclusiveArch:  x86_64

# Rust workspace.
BuildRequires:  cargo
BuildRequires:  rust
# C++ KWin effect (Qt/KWin plugin) and its build system.
BuildRequires:  gcc-c++
BuildRequires:  cmake
BuildRequires:  extra-cmake-modules
BuildRequires:  qt6-qtbase-devel
BuildRequires:  kf6-kcoreaddons-devel
BuildRequires:  kf6-kconfig-devel
BuildRequires:  kwin-devel
BuildRequires:  libepoxy-devel
# Slint GUI (winit + femtovg/OpenGL); these are the system libraries it links.
BuildRequires:  fontconfig-devel
BuildRequires:  freetype-devel
BuildRequires:  libxkbcommon-devel
BuildRequires:  wayland-devel
BuildRequires:  mesa-libGL-devel
BuildRequires:  mesa-libEGL-devel
BuildRequires:  systemd-rpm-macros

# KDE Plasma Wayland is the verified backend, so it carries the KWin effect at
# runtime. The other backends only matter on their own desktops, so they are
# weak dependencies rather than hard ones.
Recommends:     kwin-wayland
Suggests:       gnome-shell
Suggests:       gamescope

%description
Satur8 boosts color saturation for games like CS2 and restores your desktop when
you leave the game. It changes the display color pipeline after the game has
rendered, in the compositor or at scanout, so it never injects a layer, overlay,
or hook into the game process.

KDE Plasma Wayland and NVIDIA X11 are the verified backends in this release:
KDE ships a KWin saturation effect, a KWin focus-forwarder script, a focus
daemon, a CLI, a tray app, and a desktop GUI; NVIDIA X11 drives the driver's
Digital Vibrance control through NV-CONTROL. GNOME, Hyprland, DRM/KMS, and
gamescope backends are present behind environment detection but not yet
independently verified.

%prep
%autosetup -n %{name}-%{version}

%build
cargo build --release --locked

# The KWin effect ABI is not stable, so build it against this distribution's KWin
# and install it into the system Qt6 plugin path KWin scans at startup.
cmake -S assets/kwin-effect -B build-kwin \
    -DCMAKE_BUILD_TYPE=Release \
    -DVIBRANCE_PLUGIN_INSTALL_DIR=%{_qt6_plugindir}/kwin/effects/plugins
cmake --build build-kwin %{?_smp_mflags}

%install
install -Dm0755 target/release/satur8        %{buildroot}%{_bindir}/satur8
install -Dm0755 target/release/satur8-daemon %{buildroot}%{_bindir}/satur8-daemon
install -Dm0755 target/release/satur8-tray   %{buildroot}%{_bindir}/satur8-tray
install -Dm0755 target/release/satur8-gui    %{buildroot}%{_bindir}/satur8-gui

# KWin effect, in the system Qt6 plugin path.
install -Dm0755 build-kwin/satur8.so \
    %{buildroot}%{_qt6_plugindir}/kwin/effects/plugins/satur8.so

# KWin focus-forwarder script.
install -d %{buildroot}%{_datadir}/kwin/scripts/satur8-focus
cp -r assets/kwin-script/. %{buildroot}%{_datadir}/kwin/scripts/satur8-focus/

# GNOME Shell extension.
install -d %{buildroot}%{_datadir}/gnome-shell/extensions/satur8@satur8.github.io
cp -r assets/gnome-extension/satur8@satur8.github.io/. \
    %{buildroot}%{_datadir}/gnome-shell/extensions/satur8@satur8.github.io/

# gamescope fallback shader.
install -Dm0644 assets/gamescope/Satur8.fx \
    %{buildroot}%{_datadir}/gamescope/reshade/Shaders/Satur8.fx

# systemd user unit. The shipped unit points at the per-user tarball install
# (~/.local/bin); for the packaged install the daemon binary is in %{_bindir},
# so rewrite ExecStart to the packaged path. (%%h is the literal text in the
# source file, not an rpm macro.)
install -d %{buildroot}%{_userunitdir}
sed 's|%%h/.local/bin/satur8-daemon|%{_bindir}/satur8-daemon|' \
    packaging/satur8-daemon.service \
    > %{buildroot}%{_userunitdir}/satur8-daemon.service

install -Dm0644 packaging/satur8.desktop \
    %{buildroot}%{_datadir}/applications/satur8.desktop

%files
%license LICENSE
%doc README.md
%{_bindir}/satur8
%{_bindir}/satur8-daemon
%{_bindir}/satur8-tray
%{_bindir}/satur8-gui
%{_qt6_plugindir}/kwin/effects/plugins/satur8.so
%{_datadir}/kwin/scripts/satur8-focus/
%{_datadir}/gnome-shell/extensions/satur8@satur8.github.io/
%{_datadir}/gamescope/reshade/Shaders/Satur8.fx
%{_userunitdir}/satur8-daemon.service
%{_datadir}/applications/satur8.desktop

%changelog
* Wed Jun 24 2026 Satur8 <ntrpydev@pm.me> - 0.2.1-1
- Fix the NVIDIA X11 NV-CONTROL backend to set the driver's Digital Vibrance
  attribute.

* Wed Jun 24 2026 Satur8 <ntrpydev@pm.me> - 0.2.0-1
- Initial Fedora/RPM packaging for COPR.
- Builds the Rust workspace and the KWin Plasma Wayland effect from source,
  mirroring the Arch PKGBUILD layout.
