//! The gamepad: Steam Input shows the Deck's controls to a program started
//! from a Steam shortcut as an Xbox-style pad, a Linux input device. Slint
//! has no gamepad input, so this reads that device directly (`/dev/input`,
//! the kernel's evdev interface, through `libc` only) and turns what it
//! reads into the button presses `gm.rs` understands.
//!
//! The decoding and the repeat timing are pure and unit-tested. The reader
//! thread at the bottom only moves bytes: it finds the pads, feeds their
//! events to a [`Decoder`], asks the [`Repeater`] what is due, and hands each
//! [`Press`] to a callback.

use std::collections::BTreeSet;

use crate::gm::{Button, Press};

// Linux input event types and codes (linux/input-event-codes.h).
pub const EV_KEY: u16 = 0x01;
pub const EV_ABS: u16 = 0x03;
pub const BTN_SOUTH: u16 = 0x130;
pub const BTN_EAST: u16 = 0x131;
/// `BTN_NORTH`, which the kernel's Xbox pad driver and Steam's pad use for X.
pub const BTN_X: u16 = 0x133;
/// `BTN_WEST`, which they use for Y.
pub const BTN_Y: u16 = 0x134;
pub const BTN_TL: u16 = 0x136;
pub const BTN_TR: u16 = 0x137;
pub const BTN_TL2: u16 = 0x138;
pub const BTN_TR2: u16 = 0x139;
pub const BTN_DPAD_UP: u16 = 0x220;
pub const BTN_DPAD_DOWN: u16 = 0x221;
pub const BTN_DPAD_LEFT: u16 = 0x222;
pub const BTN_DPAD_RIGHT: u16 = 0x223;
pub const ABS_X: u16 = 0x00;
pub const ABS_Y: u16 = 0x01;
pub const ABS_Z: u16 = 0x02;
pub const ABS_RZ: u16 = 0x05;
pub const ABS_HAT0X: u16 = 0x10;
pub const ABS_HAT0Y: u16 = 0x11;

/// How far the left stick has to be pushed, as a share of its travel, before
/// it counts as a direction, and how far back it has to come to stop.
pub const STICK_ON: f32 = 0.6;
pub const STICK_OFF: f32 = 0.4;
/// How far a trigger has to be pulled to count as pressed.
pub const TRIGGER_ON: f32 = 0.5;

/// How long a button is held before it repeats, and how often it repeats: a
/// little slower for the first ten repeats, then faster.
pub const REPEAT_DELAY_MS: u64 = 400;
pub const REPEAT_SLOW_MS: u64 = 110;
pub const REPEAT_FAST_MS: u64 = 60;
pub const REPEAT_SLOW_COUNT: u32 = 10;

/// One event read from an input device.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct RawEvent {
    pub kind: u16,
    pub code: u16,
    pub value: i32,
}

/// The lowest and highest value an axis reports.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct AxisRange {
    pub min: i32,
    pub max: i32,
}

impl AxisRange {
    /// A stick axis of an Xbox-style pad.
    pub const STICK: AxisRange = AxisRange {
        min: -32768,
        max: 32767,
    };
    /// A trigger of an Xbox-style pad.
    pub const TRIGGER: AxisRange = AxisRange { min: 0, max: 255 };

    /// `value` as -1.0..=1.0 around the middle of the range.
    fn centered(self, value: i32) -> f32 {
        let span = (self.max as f32 - self.min as f32) / 2.0;
        if span <= 0.0 {
            return 0.0;
        }
        ((value as f32 - self.min as f32) / span - 1.0).clamp(-1.0, 1.0)
    }

    /// `value` as 0.0..=1.0 from the low end of the range.
    fn pulled(self, value: i32) -> f32 {
        let span = self.max as f32 - self.min as f32;
        if span <= 0.0 {
            return 0.0;
        }
        ((value as f32 - self.min as f32) / span).clamp(0.0, 1.0)
    }
}

/// A button going down or coming up.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Change {
    pub button: Button,
    pub down: bool,
}

/// Turns one pad's events into button changes. The D-pad (as a hat or as
/// buttons) and the left stick all count as the four directions; a direction
/// is down while any of them holds it.
#[derive(Clone, Debug)]
pub struct Decoder {
    pub stick_x: AxisRange,
    pub stick_y: AxisRange,
    pub left_trigger: AxisRange,
    pub right_trigger: AxisRange,
    stick: (f32, f32),
    stick_direction: Option<Button>,
    hat: (i32, i32),
    dpad: BTreeSet<Button>,
    trigger_axis: BTreeSet<Button>,
    keys: BTreeSet<Button>,
    /// What is held now, over all sources.
    held: BTreeSet<Button>,
}

impl Default for Decoder {
    fn default() -> Self {
        Decoder {
            stick_x: AxisRange::STICK,
            stick_y: AxisRange::STICK,
            left_trigger: AxisRange::TRIGGER,
            right_trigger: AxisRange::TRIGGER,
            stick: (0.0, 0.0),
            stick_direction: None,
            hat: (0, 0),
            dpad: BTreeSet::new(),
            trigger_axis: BTreeSet::new(),
            keys: BTreeSet::new(),
            held: BTreeSet::new(),
        }
    }
}

/// The button a key code is, if it is one this app uses.
pub fn button_for_key(code: u16) -> Option<Button> {
    Some(match code {
        BTN_SOUTH => Button::A,
        BTN_EAST => Button::B,
        BTN_X => Button::X,
        BTN_Y => Button::Y,
        BTN_TL => Button::L1,
        BTN_TR => Button::R1,
        BTN_TL2 => Button::L2,
        BTN_TR2 => Button::R2,
        BTN_DPAD_UP => Button::Up,
        BTN_DPAD_DOWN => Button::Down,
        BTN_DPAD_LEFT => Button::Left,
        BTN_DPAD_RIGHT => Button::Right,
        _ => return None,
    })
}

/// The direction a stick position means. A direction already held keeps
/// going until the stick comes back inside [`STICK_OFF`]; a new one needs
/// [`STICK_ON`]. The axis pushed further decides; up is negative Y.
pub fn stick_direction(x: f32, y: f32, held: Option<Button>) -> Option<Button> {
    let (along, direction) = if x.abs() >= y.abs() {
        (x.abs(), if x < 0.0 { Button::Left } else { Button::Right })
    } else {
        (y.abs(), if y < 0.0 { Button::Up } else { Button::Down })
    };
    let needed = if held == Some(direction) {
        STICK_OFF
    } else {
        STICK_ON
    };
    (along >= needed).then_some(direction)
}

impl Decoder {
    /// Feed one event; returns the buttons that went down or came up.
    pub fn feed(&mut self, event: RawEvent) -> Vec<Change> {
        match (event.kind, event.code) {
            (EV_KEY, code) => {
                if let Some(button) = button_for_key(code) {
                    // 0 is up; 1 is down and 2 is the kernel's own repeat.
                    if event.value == 0 {
                        self.keys.remove(&button);
                    } else {
                        self.keys.insert(button);
                    }
                }
            }
            (EV_ABS, ABS_X) => self.stick.0 = self.stick_x.centered(event.value),
            (EV_ABS, ABS_Y) => self.stick.1 = self.stick_y.centered(event.value),
            (EV_ABS, ABS_HAT0X) => self.hat.0 = event.value.signum(),
            (EV_ABS, ABS_HAT0Y) => self.hat.1 = event.value.signum(),
            (EV_ABS, ABS_Z) => self.set_trigger(
                Button::L2,
                self.left_trigger.pulled(event.value) >= TRIGGER_ON,
            ),
            (EV_ABS, ABS_RZ) => self.set_trigger(
                Button::R2,
                self.right_trigger.pulled(event.value) >= TRIGGER_ON,
            ),
            _ => return Vec::new(),
        }
        self.stick_direction = stick_direction(self.stick.0, self.stick.1, self.stick_direction);
        self.dpad.clear();
        let hat = [
            (self.hat.0, Button::Left, Button::Right),
            (self.hat.1, Button::Up, Button::Down),
        ];
        for (value, low, high) in hat {
            if value != 0 {
                self.dpad.insert(if value < 0 { low } else { high });
            }
        }
        let mut now: BTreeSet<Button> = self.keys.clone();
        now.extend(self.dpad.iter().copied());
        now.extend(self.trigger_axis.iter().copied());
        now.extend(self.stick_direction);
        let mut changes: Vec<Change> = self
            .held
            .difference(&now)
            .map(|button| Change {
                button: *button,
                down: false,
            })
            .collect();
        changes.extend(now.difference(&self.held).map(|button| Change {
            button: *button,
            down: true,
        }));
        self.held = now;
        changes
    }

    fn set_trigger(&mut self, button: Button, down: bool) {
        if down {
            self.trigger_axis.insert(button);
        } else {
            self.trigger_axis.remove(&button);
        }
    }
}

/// Whether holding this button repeats it. A, B, X and Y act once per press.
pub fn repeats(button: Button) -> bool {
    !matches!(button, Button::A | Button::B | Button::X | Button::Y)
}

/// Turns button changes and the passing of time into presses: one when a
/// button goes down, then repeats while it stays down. Only the button
/// pressed last repeats. Times are milliseconds from any fixed start.
#[derive(Clone, Debug, Default)]
pub struct Repeater {
    /// The repeating button, how often it has repeated, and when it is next due.
    held: Option<(Button, u32, u64)>,
}

impl Repeater {
    /// A button changed at `now`. Returns the press, for a button going down.
    pub fn change(&mut self, change: Change, now: u64) -> Option<Press> {
        if !change.down {
            if self
                .held
                .is_some_and(|(button, ..)| button == change.button)
            {
                self.held = None;
            }
            return None;
        }
        self.held = repeats(change.button).then_some((change.button, 0, now + REPEAT_DELAY_MS));
        Some(Press {
            button: change.button,
            repeat: 0,
        })
    }

    /// The repeat that is due at `now`, if one is. Call again until `None`:
    /// after a long gap more than one can be due.
    pub fn due(&mut self, now: u64) -> Option<Press> {
        let (button, count, at) = self.held?;
        if now < at {
            return None;
        }
        let count = count + 1;
        let interval = if count < REPEAT_SLOW_COUNT {
            REPEAT_SLOW_MS
        } else {
            REPEAT_FAST_MS
        };
        // Count from now when the look came late, so a stalled thread does
        // not fire a burst.
        let next = if at + interval <= now {
            now + interval
        } else {
            at + interval
        };
        self.held = Some((button, count, next));
        Some(Press {
            button,
            repeat: count,
        })
    }

    /// Milliseconds until the next repeat is due, for the reader's wait.
    pub fn wait(&self, now: u64) -> Option<u64> {
        self.held.map(|(_, _, at)| at.saturating_sub(now))
    }

    /// Forget what is held, for example when the pad goes away.
    pub fn release_all(&mut self) {
        self.held = None;
    }
}

/// Whether a device with these capabilities is a gamepad this app can use:
/// it has the A button and either a stick or a D-pad. `has_key` and `has_abs`
/// answer for one key or axis code.
pub fn is_gamepad(has_key: impl Fn(u16) -> bool, has_abs: impl Fn(u16) -> bool) -> bool {
    has_key(BTN_SOUTH) && (has_abs(ABS_X) || has_abs(ABS_HAT0X) || has_key(BTN_DPAD_UP))
}

// ------------------------------------------------------------------ reader

mod device {
    //! The part that touches the system: finding and reading the pads.

    use std::fs::File;
    use std::io::Read;
    use std::os::fd::AsRawFd;
    use std::os::unix::fs::OpenOptionsExt;
    use std::path::{Path, PathBuf};

    use super::{AxisRange, RawEvent};

    /// `_IOC(_IOC_READ, 'E', nr, size)` from the kernel's ioctl numbering.
    const fn read_request(nr: u64, size: u64) -> libc::c_ulong {
        ((2u64 << 30) | (size << 16) | (0x45 << 8) | nr) as libc::c_ulong
    }

    pub struct Pad {
        pub path: PathBuf,
        pub name: String,
        file: File,
        key_bits: [u8; 96],
        abs_bits: [u8; 8],
    }

    fn bit(bits: &[u8], code: u16) -> bool {
        bits.get(usize::from(code / 8))
            .is_some_and(|byte| byte & (1 << (code % 8)) != 0)
    }

    impl Pad {
        /// Open an event device for reading without blocking.
        pub fn open(path: &Path) -> std::io::Result<Pad> {
            let file = std::fs::OpenOptions::new()
                .read(true)
                .custom_flags(libc::O_NONBLOCK | libc::O_CLOEXEC)
                .open(path)?;
            let fd = file.as_raw_fd();
            let mut key_bits = [0u8; 96];
            let mut abs_bits = [0u8; 8];
            let mut name = [0u8; 128];
            // SAFETY: each ioctl writes at most the given number of bytes
            // into a buffer of that size, on a descriptor this function owns.
            unsafe {
                // EVIOCGBIT(EV_KEY, len), EVIOCGBIT(EV_ABS, len), EVIOCGNAME(len).
                libc::ioctl(
                    fd,
                    read_request(0x20 + 0x01, key_bits.len() as u64),
                    key_bits.as_mut_ptr(),
                );
                libc::ioctl(
                    fd,
                    read_request(0x20 + 0x03, abs_bits.len() as u64),
                    abs_bits.as_mut_ptr(),
                );
                libc::ioctl(fd, read_request(0x06, name.len() as u64), name.as_mut_ptr());
            }
            let end = name.iter().position(|byte| *byte == 0).unwrap_or(0);
            Ok(Pad {
                path: path.to_path_buf(),
                name: String::from_utf8_lossy(&name[..end]).into_owned(),
                file,
                key_bits,
                abs_bits,
            })
        }

        pub fn has_key(&self, code: u16) -> bool {
            bit(&self.key_bits, code)
        }

        pub fn has_abs(&self, code: u16) -> bool {
            bit(&self.abs_bits, code)
        }

        /// The range an axis reports (EVIOCGABS), if the device says.
        pub fn axis_range(&self, code: u16) -> Option<AxisRange> {
            let mut info = [0i32; 6];
            // SAFETY: EVIOCGABS writes one `struct input_absinfo`, six i32.
            let result = unsafe {
                libc::ioctl(
                    self.file.as_raw_fd(),
                    read_request(0x40 + u64::from(code), 24),
                    info.as_mut_ptr(),
                )
            };
            (result >= 0 && info[2] > info[1]).then_some(AxisRange {
                min: info[1],
                max: info[2],
            })
        }

        pub fn fd(&self) -> i32 {
            self.file.as_raw_fd()
        }

        /// Every event waiting now. `Err` means the device is gone.
        pub fn read_events(&mut self) -> std::io::Result<Vec<RawEvent>> {
            const SIZE: usize = std::mem::size_of::<libc::input_event>();
            let mut events = Vec::new();
            let mut buffer = [0u8; SIZE * 32];
            loop {
                match self.file.read(&mut buffer) {
                    Ok(0) => return Ok(events),
                    Ok(read) => {
                        for chunk in buffer[..read].chunks_exact(SIZE) {
                            // The last eight bytes: type, code, value.
                            let tail = &chunk[SIZE - 8..];
                            events.push(RawEvent {
                                kind: u16::from_ne_bytes([tail[0], tail[1]]),
                                code: u16::from_ne_bytes([tail[2], tail[3]]),
                                value: i32::from_ne_bytes([tail[4], tail[5], tail[6], tail[7]]),
                            });
                        }
                    }
                    Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                        return Ok(events)
                    }
                    Err(error) => return Err(error),
                }
            }
        }
    }

    /// The event devices present, in name order.
    pub fn event_nodes() -> Vec<PathBuf> {
        let mut nodes: Vec<PathBuf> = std::fs::read_dir("/dev/input")
            .into_iter()
            .flatten()
            .flatten()
            .map(|entry| entry.path())
            .filter(|path| {
                path.file_name()
                    .and_then(|name| name.to_str())
                    .is_some_and(|name| name.starts_with("event"))
            })
            .collect();
        nodes.sort();
        nodes
    }
}

/// Where the reader writes what it finds when asked to: stderr, and a file
/// that can be read afterwards when the app was started by Steam.
pub struct DebugLog {
    file: Option<std::fs::File>,
}

impl DebugLog {
    /// A log when `enabled`, writing to `path` as well as stderr.
    pub fn new(enabled: bool, path: Option<std::path::PathBuf>) -> Option<DebugLog> {
        if !enabled {
            return None;
        }
        let file = path.and_then(|path| {
            if let Some(dir) = path.parent() {
                let _ = std::fs::create_dir_all(dir);
            }
            std::fs::File::create(path).ok()
        });
        Some(DebugLog { file })
    }

    pub fn line(&mut self, text: &str) {
        use std::io::Write;
        eprintln!("satur8-gui input: {text}");
        if let Some(file) = self.file.as_mut() {
            let _ = writeln!(file, "{text}");
        }
    }
}

/// Start the reader thread. Each press is handed to `on_press` on that
/// thread. Every two seconds the thread looks for input devices it has not
/// seen yet, so a pad that appears later (Steam creates its own when a game
/// starts) is picked up, and one that goes away is dropped.
pub fn spawn(mut debug: Option<DebugLog>, on_press: impl Fn(Press) + Send + 'static) {
    use std::os::unix::fs::MetadataExt;
    use std::time::{Duration, Instant};

    let start = Instant::now();
    let result = std::thread::Builder::new()
        .name("gamepad".into())
        .spawn(move || {
            let mut pads: Vec<(device::Pad, Decoder)> = Vec::new();
            // Device nodes already looked at, by path and inode: a node that
            // is created again for another device has another inode.
            let mut seen: BTreeSet<(std::path::PathBuf, u64)> = BTreeSet::new();
            let mut repeater = Repeater::default();
            let mut last_scan: Option<Instant> = None;
            loop {
                if last_scan.is_none_or(|at| at.elapsed() >= Duration::from_secs(2)) {
                    last_scan = Some(Instant::now());
                    let nodes: Vec<(std::path::PathBuf, u64)> = device::event_nodes()
                        .into_iter()
                        .filter_map(|path| {
                            let inode = std::fs::metadata(&path).ok()?.ino();
                            Some((path, inode))
                        })
                        .collect();
                    seen.retain(|node| nodes.contains(node));
                    for node in nodes {
                        if !seen.insert(node.clone()) {
                            continue;
                        }
                        let path = node.0;
                        match device::Pad::open(&path) {
                            Ok(pad) => {
                                let usable = is_gamepad(|c| pad.has_key(c), |c| pad.has_abs(c));
                                if let Some(log) = debug.as_mut() {
                                    log.line(&format!(
                                        "{} \"{}\" A={} X={} Y={} stick={} hat={} dpad-buttons={}: {}",
                                        path.display(),
                                        pad.name,
                                        pad.has_key(BTN_SOUTH),
                                        pad.has_key(BTN_X),
                                        pad.has_key(BTN_Y),
                                        pad.has_abs(ABS_X),
                                        pad.has_abs(ABS_HAT0X),
                                        pad.has_key(BTN_DPAD_UP),
                                        if usable {
                                            "used as a gamepad"
                                        } else {
                                            "not a gamepad"
                                        },
                                    ));
                                }
                                if usable {
                                    let mut decoder = Decoder::default();
                                    let range =
                                        |code, fallback| pad.axis_range(code).unwrap_or(fallback);
                                    decoder.stick_x = range(ABS_X, AxisRange::STICK);
                                    decoder.stick_y = range(ABS_Y, AxisRange::STICK);
                                    decoder.left_trigger = range(ABS_Z, AxisRange::TRIGGER);
                                    decoder.right_trigger = range(ABS_RZ, AxisRange::TRIGGER);
                                    pads.push((pad, decoder));
                                }
                            }
                            Err(error) => {
                                if let Some(log) = debug.as_mut() {
                                    log.line(&format!(
                                        "{} cannot be opened: {error}",
                                        path.display()
                                    ));
                                }
                            }
                        }
                    }
                }

                let now = start.elapsed().as_millis() as u64;
                let wait = repeater.wait(now).unwrap_or(500).clamp(1, 500) as i32;
                if pads.is_empty() {
                    std::thread::sleep(Duration::from_millis(500));
                } else {
                    let mut fds: Vec<libc::pollfd> = pads
                        .iter()
                        .map(|(pad, _)| libc::pollfd {
                            fd: pad.fd(),
                            events: libc::POLLIN,
                            revents: 0,
                        })
                        .collect();
                    // SAFETY: `fds` is a valid array of `fds.len()` entries.
                    unsafe { libc::poll(fds.as_mut_ptr(), fds.len() as libc::nfds_t, wait) };
                }

                let now = start.elapsed().as_millis() as u64;
                let mut gone = Vec::new();
                for (index, (pad, decoder)) in pads.iter_mut().enumerate() {
                    match pad.read_events() {
                        Ok(events) => {
                            for event in events {
                                for change in decoder.feed(event) {
                                    if let Some(log) = debug.as_mut() {
                                        log.line(&format!(
                                            "{}: {:?} {}",
                                            pad.name,
                                            change.button,
                                            if change.down { "down" } else { "up" }
                                        ));
                                    }
                                    if let Some(press) = repeater.change(change, now) {
                                        on_press(press);
                                    }
                                }
                            }
                        }
                        Err(error) => {
                            if let Some(log) = debug.as_mut() {
                                log.line(&format!("{} went away: {error}", pad.path.display()));
                            }
                            gone.push(index);
                        }
                    }
                }
                for index in gone.into_iter().rev() {
                    pads.remove(index);
                    repeater.release_all();
                }
                // One repeat per pass keeps the UI thread from flooding.
                if let Some(press) = repeater.due(now) {
                    on_press(press);
                }
            }
        });
    if let Err(error) = result {
        eprintln!("satur8-gui: could not start the gamepad reader: {error}");
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn key(code: u16, value: i32) -> RawEvent {
        RawEvent {
            kind: EV_KEY,
            code,
            value,
        }
    }

    fn abs(code: u16, value: i32) -> RawEvent {
        RawEvent {
            kind: EV_ABS,
            code,
            value,
        }
    }

    fn down(button: Button) -> Change {
        Change { button, down: true }
    }

    fn up(button: Button) -> Change {
        Change {
            button,
            down: false,
        }
    }

    /// The Xbox-style layout the kernel's `xpad` driver and Steam's virtual
    /// pad report: A and B are BTN_SOUTH and BTN_EAST, X is BTN_NORTH (0x133)
    /// and Y is BTN_WEST (0x134).
    #[test]
    fn face_and_shoulder_buttons_decode() {
        let cases = [
            (0x130, Button::A),
            (0x131, Button::B),
            (0x133, Button::X),
            (0x134, Button::Y),
            (0x136, Button::L1),
            (0x137, Button::R1),
            (0x138, Button::L2),
            (0x139, Button::R2),
            (0x220, Button::Up),
            (0x221, Button::Down),
            (0x222, Button::Left),
            (0x223, Button::Right),
        ];
        for (code, button) in cases {
            let mut decoder = Decoder::default();
            assert_eq!(decoder.feed(key(code, 1)), vec![down(button)], "{code:#x}");
            // The kernel's own key repeat changes nothing.
            assert_eq!(decoder.feed(key(code, 2)), vec![]);
            assert_eq!(decoder.feed(key(code, 0)), vec![up(button)]);
        }
        // Select, Start, the Steam button and the stick clicks are not ours.
        for code in [0x13a, 0x13b, 0x13c, 0x13d, 0x13e, 0x132, 0x135] {
            assert_eq!(Decoder::default().feed(key(code, 1)), vec![], "{code:#x}");
        }
    }

    #[test]
    fn the_hat_is_the_dpad() {
        let mut decoder = Decoder::default();
        assert_eq!(decoder.feed(abs(ABS_HAT0Y, -1)), vec![down(Button::Up)]);
        assert_eq!(decoder.feed(abs(ABS_HAT0Y, 0)), vec![up(Button::Up)]);
        assert_eq!(decoder.feed(abs(ABS_HAT0Y, 1)), vec![down(Button::Down)]);
        assert_eq!(decoder.feed(abs(ABS_HAT0X, -1)), vec![down(Button::Left)]);
        // Straight from left to right: one up, one down.
        assert_eq!(
            decoder.feed(abs(ABS_HAT0X, 1)),
            vec![up(Button::Left), down(Button::Right)]
        );
    }

    #[test]
    fn the_left_stick_needs_a_real_push_and_lets_go_late() {
        let mut decoder = Decoder::default();
        // 50% of the travel: inside the dead zone.
        assert_eq!(decoder.feed(abs(ABS_X, 16384)), vec![]);
        // 70%: right.
        assert_eq!(decoder.feed(abs(ABS_X, 22938)), vec![down(Button::Right)]);
        // Back to 50%: still right, it only lets go below 40%.
        assert_eq!(decoder.feed(abs(ABS_X, 16384)), vec![]);
        assert_eq!(decoder.feed(abs(ABS_X, 9830)), vec![up(Button::Right)]);
        // Up is negative Y.
        assert_eq!(decoder.feed(abs(ABS_Y, -32768)), vec![down(Button::Up)]);
        assert_eq!(decoder.feed(abs(ABS_Y, 0)), vec![up(Button::Up)]);
        assert_eq!(decoder.feed(abs(ABS_Y, 32767)), vec![down(Button::Down)]);
    }

    #[test]
    fn the_axis_pushed_further_decides_a_diagonal() {
        assert_eq!(stick_direction(0.9, -0.5, None), Some(Button::Right));
        assert_eq!(stick_direction(0.5, -0.9, None), Some(Button::Up));
        assert_eq!(stick_direction(-0.7, 0.2, None), Some(Button::Left));
        assert_eq!(stick_direction(0.59, 0.0, None), None);
        assert_eq!(
            stick_direction(0.45, 0.0, Some(Button::Right)),
            Some(Button::Right)
        );
        // Holding right does not make left easier.
        assert_eq!(stick_direction(-0.45, 0.0, Some(Button::Right)), None);
        assert_eq!(stick_direction(0.39, 0.0, Some(Button::Right)), None);
    }

    #[test]
    fn stick_and_dpad_together_are_one_direction() {
        let mut decoder = Decoder::default();
        assert_eq!(decoder.feed(abs(ABS_HAT0X, 1)), vec![down(Button::Right)]);
        // The stick joins in: nothing new.
        assert_eq!(decoder.feed(abs(ABS_X, 32767)), vec![]);
        // The D-pad lets go, the stick still holds it.
        assert_eq!(decoder.feed(abs(ABS_HAT0X, 0)), vec![]);
        assert_eq!(decoder.feed(abs(ABS_X, 0)), vec![up(Button::Right)]);
    }

    #[test]
    fn triggers_press_at_half_way_with_the_devices_own_range() {
        let mut decoder = Decoder::default();
        assert_eq!(decoder.feed(abs(ABS_Z, 100)), vec![]);
        assert_eq!(decoder.feed(abs(ABS_Z, 128)), vec![down(Button::L2)]);
        assert_eq!(decoder.feed(abs(ABS_Z, 255)), vec![]);
        assert_eq!(decoder.feed(abs(ABS_Z, 0)), vec![up(Button::L2)]);
        // A pad whose triggers go to 1023.
        decoder.right_trigger = AxisRange { min: 0, max: 1023 };
        assert_eq!(decoder.feed(abs(ABS_RZ, 255)), vec![]);
        assert_eq!(decoder.feed(abs(ABS_RZ, 600)), vec![down(Button::R2)]);
    }

    #[test]
    fn other_events_are_ignored() {
        let mut decoder = Decoder::default();
        // The right stick, a sync event, a misc event.
        assert_eq!(decoder.feed(abs(0x03, 32767)), vec![]);
        assert_eq!(decoder.feed(abs(0x04, -32768)), vec![]);
        for kind in [0x00, 0x04] {
            let event = RawEvent {
                kind,
                code: 0,
                value: 1,
            };
            assert_eq!(decoder.feed(event), vec![]);
        }
    }

    fn press_of(button: Button, repeat: u32) -> Option<Press> {
        Some(Press { button, repeat })
    }

    #[test]
    fn a_held_direction_repeats_after_a_delay_then_speeds_up() {
        let mut repeater = Repeater::default();
        assert_eq!(
            repeater.change(down(Button::Down), 1000),
            press_of(Button::Down, 0)
        );
        assert_eq!(repeater.due(1000), None);
        assert_eq!(repeater.due(1399), None);
        assert_eq!(repeater.wait(1100), Some(300));
        assert_eq!(repeater.due(1400), press_of(Button::Down, 1));
        assert_eq!(repeater.due(1400), None);
        assert_eq!(repeater.due(1509), None);
        assert_eq!(repeater.due(1510), press_of(Button::Down, 2));
        // Repeats 3 to 9 at the slow rate, then the fast one.
        let mut at = 1510;
        for count in 3..=9 {
            at += REPEAT_SLOW_MS;
            assert_eq!(repeater.due(at), press_of(Button::Down, count));
        }
        at += REPEAT_SLOW_MS;
        assert_eq!(repeater.due(at), press_of(Button::Down, 10));
        assert_eq!(repeater.due(at + REPEAT_FAST_MS - 1), None);
        assert_eq!(
            repeater.due(at + REPEAT_FAST_MS),
            press_of(Button::Down, 11)
        );
    }

    #[test]
    fn letting_go_stops_the_repeat() {
        let mut repeater = Repeater::default();
        repeater.change(down(Button::Left), 0);
        assert_eq!(repeater.change(up(Button::Left), 200), None);
        assert_eq!(repeater.due(5000), None);
        assert_eq!(repeater.wait(5000), None);
    }

    #[test]
    fn only_the_button_pressed_last_repeats() {
        let mut repeater = Repeater::default();
        repeater.change(down(Button::Down), 0);
        assert_eq!(
            repeater.change(down(Button::Right), 100),
            press_of(Button::Right, 0)
        );
        // Letting go of the earlier one changes nothing.
        repeater.change(up(Button::Down), 150);
        assert_eq!(repeater.due(500), press_of(Button::Right, 1));
    }

    #[test]
    fn the_face_buttons_act_once_however_long_they_are_held() {
        for button in [Button::A, Button::B, Button::X, Button::Y] {
            let mut repeater = Repeater::default();
            assert_eq!(repeater.change(down(button), 0), press_of(button, 0));
            assert_eq!(repeater.due(10_000), None, "{button:?}");
        }
        // And pressing one stops a direction from repeating under it.
        let mut repeater = Repeater::default();
        repeater.change(down(Button::Down), 0);
        repeater.change(down(Button::A), 100);
        assert_eq!(repeater.due(10_000), None);
    }

    #[test]
    fn a_stalled_reader_does_not_fire_a_burst() {
        let mut repeater = Repeater::default();
        repeater.change(down(Button::Down), 0);
        // Five seconds without a look: one repeat now, the next an interval on.
        assert_eq!(repeater.due(5000), press_of(Button::Down, 1));
        assert_eq!(repeater.due(5000), None);
        assert_eq!(
            repeater.due(5000 + REPEAT_SLOW_MS),
            press_of(Button::Down, 2)
        );
    }

    #[test]
    fn a_gamepad_has_the_a_button_and_a_way_to_point() {
        let keys = |list: &'static [u16]| move |code: u16| list.contains(&code);
        // Steam's virtual pad and the kernel's Xbox pad.
        assert!(is_gamepad(
            keys(&[BTN_SOUTH, BTN_EAST]),
            keys(&[ABS_X, ABS_Y, ABS_HAT0X])
        ));
        // A pad with only a D-pad.
        assert!(is_gamepad(keys(&[BTN_SOUTH]), keys(&[ABS_HAT0X])));
        assert!(is_gamepad(keys(&[BTN_SOUTH, BTN_DPAD_UP]), keys(&[])));
        // A mouse, a keyboard, a touchscreen with absolute axes.
        assert!(!is_gamepad(keys(&[0x110, 0x111]), keys(&[])));
        assert!(!is_gamepad(keys(&[30, 31, 32]), keys(&[])));
        assert!(!is_gamepad(keys(&[0x14a]), keys(&[ABS_X, ABS_Y])));
        // Buttons but nothing to point with.
        assert!(!is_gamepad(keys(&[BTN_SOUTH]), keys(&[])));
    }

    #[test]
    fn axis_ranges_scale_to_unit_values() {
        assert_eq!(AxisRange::STICK.centered(-32768), -1.0);
        assert!(AxisRange::STICK.centered(0).abs() < 0.001);
        assert_eq!(AxisRange::STICK.centered(32767), 1.0);
        assert_eq!(AxisRange::TRIGGER.pulled(0), 0.0);
        assert_eq!(AxisRange::TRIGGER.pulled(255), 1.0);
        // A range that says nothing gives nothing.
        let broken = AxisRange { min: 5, max: 5 };
        assert_eq!(broken.centered(5), 0.0);
        assert_eq!(broken.pulled(5), 0.0);
    }
}
