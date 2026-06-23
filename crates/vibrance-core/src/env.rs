//! Environment detection: which display session, desktop, and GPU we're on.
//!
//! This drives backend selection (PLAN.md section 5). Detection is cheap and
//! done once at startup - never polled.

use std::fmt;

/// The display server in use. This decides *who owns DRM* and therefore which
/// backends can reach the color pipeline at all.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SessionType {
    Wayland,
    X11,
    /// Bare KMS / no display server - we can own DRM master ourselves.
    Tty,
    Unknown,
}

/// The Wayland compositor / desktop, where it matters for backend choice.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Desktop {
    Kde,
    Gnome,
    Hyprland,
    Sway,
    Other,
}

/// GPU vendor, which decides whether the hardware CTM is reachable and reliable.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Gpu {
    Amd,
    Intel,
    /// NVIDIA proprietary driver (CTM unreliable; use NV-CONTROL on X11).
    Nvidia,
    /// Open nouveau driver (exposes DRM CTM).
    Nouveau,
    Unknown,
}

/// A snapshot of the runtime environment.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Environment {
    pub session: SessionType,
    pub desktop: Desktop,
    pub gpu: Gpu,
}

impl Environment {
    /// Detect from the process environment and (for the GPU) the DRM sysfs tree.
    pub fn detect() -> Environment {
        Environment {
            session: detect_session(),
            desktop: detect_desktop(),
            gpu: detect_gpu(),
        }
    }

    /// The backend that should be preferred here, by name, following the
    /// selection order in PLAN.md. The actual backend crates still confirm
    /// availability with their own `detect()`; this is the intended pick.
    pub fn preferred_backend(self) -> &'static str {
        match (self.session, self.desktop) {
            (SessionType::Wayland, Desktop::Kde) => "kwin",
            (SessionType::Wayland, Desktop::Hyprland) => "hyprland",
            (SessionType::Wayland, Desktop::Gnome) => "gnome-shell",
            (SessionType::Wayland, _) => "gamescope",
            (SessionType::X11, _) => match self.gpu {
                Gpu::Nvidia => "nv-control",
                _ => "drm-ctm",
            },
            (SessionType::Tty, _) => "drm-ctm",
            (SessionType::Unknown, _) => "gamescope",
        }
    }
}

fn env(key: &str) -> Option<String> {
    std::env::var(key).ok().filter(|v| !v.is_empty())
}

fn detect_session() -> SessionType {
    if let Some(t) = env("XDG_SESSION_TYPE") {
        match t.as_str() {
            "wayland" => return SessionType::Wayland,
            "x11" => return SessionType::X11,
            "tty" => return SessionType::Tty,
            _ => {}
        }
    }
    if env("WAYLAND_DISPLAY").is_some() {
        SessionType::Wayland
    } else if env("DISPLAY").is_some() {
        SessionType::X11
    } else {
        SessionType::Unknown
    }
}

fn detect_desktop() -> Desktop {
    // Hyprland and Sway advertise themselves with their own env vars first.
    if env("HYPRLAND_INSTANCE_SIGNATURE").is_some() {
        return Desktop::Hyprland;
    }
    if env("SWAYSOCK").is_some() {
        return Desktop::Sway;
    }
    let desk = env("XDG_CURRENT_DESKTOP")
        .or_else(|| env("XDG_SESSION_DESKTOP"))
        .or_else(|| env("DESKTOP_SESSION"))
        .unwrap_or_default()
        .to_ascii_lowercase();
    if desk.contains("kde") || desk.contains("plasma") {
        Desktop::Kde
    } else if desk.contains("gnome") {
        Desktop::Gnome
    } else if desk.contains("hyprland") {
        Desktop::Hyprland
    } else if desk.contains("sway") {
        Desktop::Sway
    } else {
        Desktop::Other
    }
}

fn detect_gpu() -> Gpu {
    // Read the bound driver from DRM sysfs. On a hybrid box (iGPU + discrete) the
    // card driving the display is the one we care about, so prefer a card that
    // has a *connected* connector; fall back to the first render card otherwise.
    let cards: Vec<(String, std::path::PathBuf)> = match std::fs::read_dir("/sys/class/drm") {
        Ok(entries) => entries
            .flatten()
            .filter_map(|e| {
                let name = e.file_name().to_string_lossy().to_string();
                // Match "card0", "card1", ... but not "card0-DP-1" connector nodes.
                let is_card = name.starts_with("card")
                    && name.len() > 4
                    && name[4..].chars().all(|c| c.is_ascii_digit());
                is_card.then(|| (name, e.path()))
            })
            .collect(),
        Err(_) => return Gpu::Unknown,
    };

    let driver_of = |path: &std::path::Path| -> Option<String> {
        std::fs::read_link(path.join("device/driver")).ok().map(|t| {
            t.file_name()
                .map(|n| n.to_string_lossy().to_string())
                .unwrap_or_default()
        })
    };
    let classify = |driver: &str| -> Option<Gpu> {
        match driver {
            "amdgpu" | "radeon" => Some(Gpu::Amd),
            "i915" | "xe" => Some(Gpu::Intel),
            "nvidia" | "nvidia-drm" => Some(Gpu::Nvidia),
            "nouveau" => Some(Gpu::Nouveau),
            _ => None,
        }
    };

    // First choice: a card with at least one connected connector.
    for (name, path) in &cards {
        let has_connected = std::fs::read_dir("/sys/class/drm")
            .into_iter()
            .flatten()
            .flatten()
            .filter_map(|e| {
                let n = e.file_name().to_string_lossy().to_string();
                n.starts_with(&format!("{name}-")).then_some(e.path())
            })
            .any(|conn| {
                std::fs::read_to_string(conn.join("status"))
                    .map(|s| s.trim() == "connected")
                    .unwrap_or(false)
            });
        if has_connected {
            if let Some(gpu) = driver_of(path).as_deref().and_then(classify) {
                return gpu;
            }
        }
    }

    // Fallback: first render-capable card we can classify.
    for (_, path) in &cards {
        if let Some(gpu) = driver_of(path).as_deref().and_then(classify) {
            return gpu;
        }
    }
    Gpu::Unknown
}

impl fmt::Display for SessionType {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let s = match self {
            SessionType::Wayland => "Wayland",
            SessionType::X11 => "X11",
            SessionType::Tty => "TTY/KMS",
            SessionType::Unknown => "unknown",
        };
        f.write_str(s)
    }
}

impl fmt::Display for Desktop {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let s = match self {
            Desktop::Kde => "KDE Plasma",
            Desktop::Gnome => "GNOME",
            Desktop::Hyprland => "Hyprland",
            Desktop::Sway => "Sway",
            Desktop::Other => "other",
        };
        f.write_str(s)
    }
}

impl fmt::Display for Gpu {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let s = match self {
            Gpu::Amd => "AMD",
            Gpu::Intel => "Intel",
            Gpu::Nvidia => "NVIDIA (proprietary)",
            Gpu::Nouveau => "NVIDIA (nouveau)",
            Gpu::Unknown => "unknown",
        };
        f.write_str(s)
    }
}
