//! The Steam Game Mode screen: which element is highlighted, what each of the
//! Deck's buttons does there, and what the prompt bar says. Everything here is
//! pure: `main.rs` feeds it button presses (from the gamepad reader in
//! `pad.rs` or from the keyboard) and touches, carries out the [`Effect`]s it
//! returns, and hands the state to `ui/touch.slint` to draw.
//!
//! One element is in focus at a time. The highlight is drawn only while the
//! pad is in use: a touch moves the focus to what was touched and hides the
//! highlight, and the next button press shows it again without doing anything
//! else, the way console interfaces do.

/// The three pages, in tab order.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Page {
    Games,
    Status,
    About,
}

impl Page {
    pub fn index(self) -> i32 {
        match self {
            Page::Games => 0,
            Page::Status => 1,
            Page::About => 2,
        }
    }

    pub fn from_index(index: i32) -> Page {
        match index {
            1 => Page::Status,
            2 => Page::About,
            _ => Page::Games,
        }
    }
}

/// The parts of a game's row, left to right.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Part {
    Slider,
    Toggle,
    Remove,
}

/// The parts of the "Everything else" row, left to right.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RestPart {
    Slider,
    Reset,
}

/// The element in focus.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Focus {
    /// The first row: every game without a row of its own.
    Rest(RestPart),
    /// The "Add a game" button.
    AddGame,
    /// A game's row.
    Game(usize, Part),
    /// The question a row asks before it is removed: Keep (`false`) or
    /// Remove (`true`).
    Confirm(usize, bool),
    /// A row of the open add-a-game list.
    Pick(usize),
    /// An item of the Status or About page, in the order of its [`Layout`].
    Item(usize),
}

/// One focusable item of the Status or About page.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Item {
    Button {
        enabled: bool,
    },
    Toggle {
        enabled: bool,
    },
    /// The "starting saturation for new games" slider.
    Slider,
}

/// What the screen holds right now: all the focus model needs to know.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Layout {
    /// Percent (-100..=100) of the "Everything else" row.
    pub rest_percent: i32,
    /// Each game row's percent and whether it is turned on.
    pub games: Vec<(i32, bool)>,
    /// How many rows the open add-a-game list has.
    pub picks: usize,
    /// The Status page's items.
    pub status: Vec<Item>,
    /// The About page's items.
    pub about: Vec<Item>,
    /// Percent of the "starting saturation for new games" slider.
    pub new_game_percent: i32,
}

/// The Deck's buttons, by what Steam's gamepad layout calls them.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum Button {
    Up,
    Down,
    Left,
    Right,
    A,
    B,
    X,
    Y,
    L1,
    R1,
    L2,
    R2,
}

/// The button a key stands for, so the screen also works from a keyboard and
/// from a Steam controller layout that sends keys (the "Web Browser" template
/// sends the arrow keys and Return). `text` is Slint's key text.
pub fn button_for_key(text: &str) -> Option<Button> {
    Some(match text {
        "\u{F700}" => Button::Up,
        "\u{F701}" => Button::Down,
        "\u{F702}" => Button::Left,
        "\u{F703}" => Button::Right,
        "\n" | " " => Button::A,
        "\u{1b}" | "\u{8}" => Button::B,
        "x" | "X" => Button::X,
        "y" | "Y" => Button::Y,
        "\u{F72C}" | "q" | "Q" => Button::L1,
        "\u{F72D}" | "e" | "E" => Button::R1,
        "-" | "[" => Button::L2,
        "+" | "=" | "]" => Button::R2,
        _ => return None,
    })
}

/// One press: the first (`repeat` 0) or a repeat while the button is held.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Press {
    pub button: Button,
    pub repeat: u32,
}

/// A slider that can be changed.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Slider {
    Rest,
    Game(usize),
    NewGame,
}

/// What a press asks `main.rs` to do.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Effect {
    /// Set a slider. `done` means the change is finished and is saved now;
    /// otherwise it is saved once the changes stop.
    Percent {
        slider: Slider,
        value: i32,
        done: bool,
    },
    ToggleGame(usize),
    RemoveGame(usize),
    OpenPicker,
    ClosePicker,
    PickRow(usize),
    ShowPage(Page),
    /// A on a Status or About button or toggle.
    PressItem(usize),
}

/// The Game Mode screen's state.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct State {
    pub page: Page,
    pub focus: Focus,
    /// While a slider is being changed with left and right: the value it had
    /// when that began, which B puts back.
    pub adjusting: Option<i32>,
    pub picker_open: bool,
    /// Whether the highlight is drawn: true after pad input, false after a
    /// touch.
    pub highlight: bool,
}

/// How far one left or right press moves a slider: 1 at first, more the
/// longer the direction is held.
pub fn slider_step(repeat: u32) -> i32 {
    match repeat {
        0..=5 => 1,
        6..=15 => 2,
        _ => 5,
    }
}

fn clamp_percent(value: i32) -> i32 {
    value.clamp(-100, 100)
}

impl State {
    /// The state the app opens in: on the first game's slider, or on the
    /// "Add a game" button when there are no games yet.
    pub fn new(layout: &Layout) -> State {
        State {
            page: Page::Games,
            focus: home_focus(Page::Games, layout),
            adjusting: None,
            picker_open: false,
            highlight: false,
        }
    }

    /// The slider in focus, if the focus is on one.
    fn focused_slider(&self) -> Option<Slider> {
        match (self.page, self.focus) {
            (Page::Games, Focus::Rest(RestPart::Slider)) => Some(Slider::Rest),
            (Page::Games, Focus::Game(index, Part::Slider)) => Some(Slider::Game(index)),
            _ => None,
        }
    }

    /// The slider of the row in focus, whichever part of the row has it:
    /// what L2 and R2 change.
    fn row_slider(&self, layout: &Layout) -> Option<Slider> {
        match (self.page, self.focus) {
            (Page::Games, Focus::Rest(_)) => Some(Slider::Rest),
            (Page::Games, Focus::Game(index, _)) => Some(Slider::Game(index)),
            (Page::Status, Focus::Item(index)) => {
                (layout.status.get(index) == Some(&Item::Slider)).then_some(Slider::NewGame)
            }
            _ => None,
        }
    }

    /// A touch landed on `focus`: it takes the focus, the highlight hides,
    /// and a slider change in progress with the pad is over.
    pub fn touched(&mut self, focus: Focus) {
        self.focus = focus;
        self.highlight = false;
        self.adjusting = None;
    }

    /// A page was opened by touch.
    pub fn page_touched(&mut self, page: Page, layout: &Layout) {
        self.page = page;
        self.picker_open = false;
        self.touched(home_focus(page, layout));
    }

    /// The add-a-game list was opened by touch.
    pub fn picker_opened_by_touch(&mut self) {
        self.page = Page::Games;
        self.picker_open = true;
        self.touched(Focus::Pick(0));
    }

    /// The add-a-game list was closed without a pick.
    pub fn picker_closed(&mut self) {
        self.picker_open = false;
        self.adjusting = None;
        self.focus = Focus::AddGame;
    }

    /// A row of the list was picked and the profile at `index` is shown:
    /// the focus lands on its slider. With the pad in use the slider is
    /// ready to change with left and right; `percent` is its value.
    pub fn picked(&mut self, index: usize, percent: i32) {
        self.page = Page::Games;
        self.picker_open = false;
        self.focus = Focus::Game(index, Part::Slider);
        self.adjusting = self.highlight.then_some(percent);
    }

    /// The game at `index` was removed; `remaining` games are left.
    pub fn removed(&mut self, index: usize, remaining: usize) {
        self.adjusting = None;
        self.focus = if remaining == 0 {
            Focus::AddGame
        } else {
            Focus::Game(index.min(remaining - 1), Part::Remove)
        };
    }

    /// Keep the focus on something that exists after the screen changed
    /// under it (profiles reloaded, list filtered).
    pub fn settle(&mut self, layout: &Layout) {
        let games = layout.games.len();
        match self.focus {
            Focus::Game(index, _) | Focus::Confirm(index, _) if index >= games => {
                self.adjusting = None;
                self.focus = home_focus(Page::Games, layout);
            }
            Focus::Pick(index) if index >= layout.picks => {
                self.focus = Focus::Pick(layout.picks.saturating_sub(1));
            }
            Focus::Item(index) => {
                let count = match self.page {
                    Page::Status => layout.status.len(),
                    _ => layout.about.len(),
                };
                if index >= count {
                    self.focus = Focus::Item(count.saturating_sub(1));
                }
            }
            _ => {}
        }
    }
}

/// Where the focus starts on a page.
fn home_focus(page: Page, layout: &Layout) -> Focus {
    match page {
        Page::Games if layout.games.is_empty() => Focus::AddGame,
        Page::Games => Focus::Game(0, Part::Slider),
        Page::Status | Page::About => Focus::Item(0),
    }
}

fn slider_value(layout: &Layout, slider: Slider) -> i32 {
    match slider {
        Slider::Rest => layout.rest_percent,
        Slider::Game(index) => layout.games.get(index).map_or(0, |game| game.0),
        Slider::NewGame => layout.new_game_percent,
    }
}

/// A button was pressed. Changes `state` and returns what to do.
pub fn press(state: &mut State, layout: &Layout, press: Press) -> Vec<Effect> {
    use Button::*;
    let button = press.button;
    if !state.highlight {
        // First pad input after a touch: show where the focus is. Buttons
        // that act on the element in focus do nothing more this time, since
        // the user could not see what that element was.
        state.highlight = true;
        if matches!(button, Up | Down | Left | Right | A | X | L2 | R2) {
            return Vec::new();
        }
    }
    let mut effects = Vec::new();

    if state.picker_open {
        let last = layout.picks.saturating_sub(1);
        let at = match state.focus {
            Focus::Pick(index) => index.min(last),
            _ => 0,
        };
        match button {
            Up => state.focus = Focus::Pick(at.saturating_sub(1)),
            Down => state.focus = Focus::Pick((at + 1).min(last)),
            L1 => state.focus = Focus::Pick(at.saturating_sub(5)),
            R1 => state.focus = Focus::Pick((at + 5).min(last)),
            A if layout.picks > 0 && press.repeat == 0 => effects.push(Effect::PickRow(at)),
            B | Y if press.repeat == 0 => {
                state.picker_closed();
                effects.push(Effect::ClosePicker);
            }
            _ => {}
        }
        return effects;
    }

    if let Focus::Confirm(index, remove) = state.focus {
        match button {
            Left => state.focus = Focus::Confirm(index, false),
            Right => state.focus = Focus::Confirm(index, true),
            A if press.repeat == 0 => {
                if remove {
                    effects.push(Effect::RemoveGame(index));
                } else {
                    state.focus = Focus::Game(index, Part::Remove);
                }
            }
            B if press.repeat == 0 => state.focus = Focus::Game(index, Part::Remove),
            _ => {}
        }
        return effects;
    }

    // L2 and R2 change the row's slider by one, in or out of adjusting.
    if matches!(button, L2 | R2) {
        if let Some(slider) = state.row_slider(layout) {
            let delta = if button == L2 { -1 } else { 1 };
            effects.push(Effect::Percent {
                slider,
                value: clamp_percent(slider_value(layout, slider) + delta),
                done: false,
            });
        }
        return effects;
    }

    if let Some(start) = state.adjusting {
        let slider = state.row_slider(layout);
        match (button, slider) {
            (Left | Right, Some(slider)) => {
                let step = slider_step(press.repeat);
                let delta = if button == Left { -step } else { step };
                effects.push(Effect::Percent {
                    slider,
                    value: clamp_percent(slider_value(layout, slider) + delta),
                    done: false,
                });
                return effects;
            }
            (A, Some(slider)) => {
                if press.repeat == 0 {
                    state.adjusting = None;
                    effects.push(Effect::Percent {
                        slider,
                        value: slider_value(layout, slider),
                        done: true,
                    });
                }
                return effects;
            }
            (B, Some(slider)) => {
                if press.repeat == 0 {
                    state.adjusting = None;
                    effects.push(Effect::Percent {
                        slider,
                        value: start,
                        done: true,
                    });
                }
                return effects;
            }
            (_, slider) => {
                // Anything else finishes the change and then does its own job.
                state.adjusting = None;
                if let Some(slider) = slider {
                    effects.push(Effect::Percent {
                        slider,
                        value: slider_value(layout, slider),
                        done: true,
                    });
                }
            }
        }
    }

    match button {
        L1 | R1 => {
            if press.repeat == 0 {
                let next = (state.page.index() + if button == L1 { 2 } else { 1 }) % 3;
                state.page = Page::from_index(next);
                state.focus = home_focus(state.page, layout);
                effects.push(Effect::ShowPage(state.page));
            }
        }
        Y => {
            if press.repeat == 0 {
                if state.page != Page::Games {
                    state.page = Page::Games;
                    effects.push(Effect::ShowPage(Page::Games));
                }
                state.picker_open = true;
                state.focus = Focus::Pick(0);
                effects.push(Effect::OpenPicker);
            }
        }
        _ => match state.page {
            Page::Games => press_on_games(state, layout, press, &mut effects),
            Page::Status => press_on_list(state, layout, &layout.status, press, &mut effects),
            Page::About => press_on_list(state, layout, &layout.about, press, &mut effects),
        },
    }
    effects
}

fn press_on_games(state: &mut State, layout: &Layout, press: Press, effects: &mut Vec<Effect>) {
    use Button::*;
    let games = layout.games.len();
    let first = press.repeat == 0;
    state.focus = match (state.focus, press.button) {
        // Rows, top to bottom: Everything else, Add a game, then the games.
        (Focus::Rest(_), Down) => Focus::AddGame,
        (Focus::AddGame, Up) => Focus::Rest(RestPart::Slider),
        (Focus::AddGame, Down) if games > 0 => Focus::Game(0, Part::Slider),
        (Focus::Game(0, _), Up) => Focus::AddGame,
        (Focus::Game(index, part), Up) => Focus::Game(index - 1, part),
        (Focus::Game(index, part), Down) if index + 1 < games => Focus::Game(index + 1, part),
        // Within a row, left to right.
        (Focus::Rest(RestPart::Slider), Right) => Focus::Rest(RestPart::Reset),
        (Focus::Rest(RestPart::Reset), Left) => Focus::Rest(RestPart::Slider),
        (Focus::Game(index, Part::Slider), Right) => Focus::Game(index, Part::Toggle),
        (Focus::Game(index, Part::Toggle), Right) => Focus::Game(index, Part::Remove),
        (Focus::Game(index, Part::Remove), Left) => Focus::Game(index, Part::Toggle),
        (Focus::Game(index, Part::Toggle), Left) => Focus::Game(index, Part::Slider),
        (Focus::Game(index, _), X) if first => {
            effects.push(Effect::ToggleGame(index));
            state.focus
        }
        (focus, A) if first => match focus {
            Focus::Rest(RestPart::Slider) | Focus::Game(_, Part::Slider) => {
                if let Some(slider) = state.focused_slider() {
                    state.adjusting = Some(slider_value(layout, slider));
                }
                focus
            }
            Focus::Rest(RestPart::Reset) => {
                effects.push(Effect::Percent {
                    slider: Slider::Rest,
                    value: 0,
                    done: true,
                });
                focus
            }
            Focus::AddGame => {
                state.picker_open = true;
                effects.push(Effect::OpenPicker);
                Focus::Pick(0)
            }
            Focus::Game(index, Part::Toggle) => {
                effects.push(Effect::ToggleGame(index));
                focus
            }
            // Removing asks first, and the answer starts on Keep.
            Focus::Game(index, Part::Remove) => Focus::Confirm(index, false),
            other => other,
        },
        // A focus left over from another page.
        (Focus::Pick(_) | Focus::Item(_), _) => home_focus(Page::Games, layout),
        (focus, _) => focus,
    };
}

/// A page that is a plain list of items: up and left go to the one before,
/// down and right to the one after, B goes back to the games.
fn press_on_list(
    state: &mut State,
    layout: &Layout,
    items: &[Item],
    press: Press,
    effects: &mut Vec<Effect>,
) {
    use Button::*;
    let last = items.len().saturating_sub(1);
    let at = match state.focus {
        Focus::Item(index) => index.min(last),
        _ => 0,
    };
    let first = press.repeat == 0;
    match press.button {
        Up | Left => state.focus = Focus::Item(at.saturating_sub(1)),
        Down | Right => state.focus = Focus::Item((at + 1).min(last)),
        A if first => match items.get(at) {
            Some(Item::Button { enabled: true }) | Some(Item::Toggle { enabled: true }) => {
                effects.push(Effect::PressItem(at));
            }
            Some(Item::Slider) => state.adjusting = Some(layout.new_game_percent),
            _ => {}
        },
        B if first => {
            state.page = Page::Games;
            state.focus = home_focus(Page::Games, layout);
            effects.push(Effect::ShowPage(Page::Games));
        }
        _ => {
            state.focus = Focus::Item(at);
        }
    }
}

/// One entry of the prompt bar: the button glyphs and what they do now.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Prompt {
    /// One or two glyph names, for example `["A"]` or `["L1", "R1"]`.
    pub glyphs: Vec<&'static str>,
    pub label: String,
}

fn prompt(glyphs: &[&'static str], label: &str) -> Prompt {
    Prompt {
        glyphs: glyphs.to_vec(),
        label: label.to_string(),
    }
}

/// What the prompt bar shows for this state.
pub fn prompts(state: &State, layout: &Layout) -> Vec<Prompt> {
    let pages = prompt(&["L1", "R1"], "Pages");
    let add = prompt(&["Y"], "Add a game");
    if state.picker_open {
        let mut bar = Vec::new();
        if layout.picks > 0 && state.highlight {
            bar.push(prompt(&["A"], "Pick"));
        }
        bar.push(prompt(&["B"], "Close"));
        if layout.picks > 5 {
            bar.push(prompt(&["L1", "R1"], "Jump"));
        }
        return bar;
    }
    if !state.highlight {
        // Touch is in use: only what works without a visible focus.
        return match state.page {
            Page::Games => vec![add, pages],
            _ => vec![prompt(&["B"], "Games"), pages],
        };
    }
    if let Focus::Confirm(..) = state.focus {
        return vec![prompt(&["A"], "Choose"), prompt(&["B"], "Keep")];
    }
    if state.adjusting.is_some() {
        return vec![
            prompt(&["\u{25c0}", "\u{25b6}"], "Change"),
            prompt(&["L2", "R2"], "By 1"),
            prompt(&["A"], "Done"),
            prompt(&["B"], "Undo"),
        ];
    }
    let fine = prompt(&["L2", "R2"], "Change");
    match (state.page, state.focus) {
        (Page::Games, Focus::Rest(RestPart::Slider)) => {
            vec![prompt(&["A"], "Change"), fine, add, pages]
        }
        (Page::Games, Focus::Rest(RestPart::Reset)) => {
            vec![prompt(&["A"], "Back to 0%"), fine, add, pages]
        }
        (Page::Games, Focus::AddGame) => vec![prompt(&["A"], "Add a game"), pages],
        (Page::Games, Focus::Game(index, part)) => {
            let on = layout.games.get(index).is_some_and(|game| game.1);
            let switch = if on { "Turn off" } else { "Turn on" };
            let mut bar = vec![match part {
                Part::Slider => prompt(&["A"], "Change"),
                Part::Toggle => prompt(&["A"], switch),
                Part::Remove => prompt(&["A"], "Remove"),
            }];
            if part != Part::Toggle {
                bar.push(prompt(&["X"], switch));
            }
            bar.extend([fine, add, pages]);
            bar
        }
        (Page::Games, _) => vec![add, pages],
        (page, Focus::Item(index)) => {
            let items = if page == Page::Status {
                &layout.status
            } else {
                &layout.about
            };
            let mut bar = Vec::new();
            match items.get(index) {
                Some(Item::Button { enabled: true }) => bar.push(prompt(&["A"], "Select")),
                Some(Item::Toggle { enabled: true }) => bar.push(prompt(&["A"], "Switch")),
                Some(Item::Slider) => bar.extend([prompt(&["A"], "Change"), fine]),
                _ => {}
            }
            bar.extend([prompt(&["B"], "Games"), pages]);
            bar
        }
        _ => vec![prompt(&["B"], "Games"), pages],
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use Button::*;

    fn layout(games: usize) -> Layout {
        Layout {
            rest_percent: 0,
            games: (0..games).map(|i| (10 * i as i32 + 40, true)).collect(),
            picks: 10,
            status: vec![
                Item::Button { enabled: true },
                Item::Button { enabled: true },
                Item::Toggle { enabled: true },
                Item::Slider,
            ],
            about: vec![Item::Button { enabled: true }; 3],
            new_game_percent: 40,
        }
    }

    fn with_pad(layout: &Layout) -> State {
        State {
            highlight: true,
            ..State::new(layout)
        }
    }

    fn tap(state: &mut State, layout: &Layout, button: Button) -> Vec<Effect> {
        press(state, layout, Press { button, repeat: 0 })
    }

    fn hold(state: &mut State, layout: &Layout, button: Button, repeat: u32) -> Vec<Effect> {
        press(state, layout, Press { button, repeat })
    }

    #[test]
    fn opens_on_the_first_game_or_on_add_when_there_are_none() {
        assert_eq!(State::new(&layout(2)).focus, Focus::Game(0, Part::Slider));
        assert_eq!(State::new(&layout(0)).focus, Focus::AddGame);
        assert!(!State::new(&layout(2)).highlight);
    }

    #[test]
    fn the_first_press_after_a_touch_only_shows_the_highlight() {
        let l = layout(2);
        for button in [Up, Down, Left, Right, A, X, L2, R2] {
            let mut state = State::new(&l);
            let before = state.focus;
            assert_eq!(tap(&mut state, &l, button), vec![], "{button:?}");
            assert!(state.highlight);
            assert_eq!(state.focus, before, "{button:?}");
            assert_eq!(state.adjusting, None);
        }
        // Buttons that do not depend on the focus act at once.
        let mut state = State::new(&l);
        assert_eq!(tap(&mut state, &l, Y), vec![Effect::OpenPicker]);
        let mut state = State::new(&l);
        assert_eq!(
            tap(&mut state, &l, R1),
            vec![Effect::ShowPage(Page::Status)]
        );
    }

    /// Focus, button, new focus: up and down through the rows.
    #[test]
    fn up_and_down_move_through_the_rows_and_stop_at_the_ends() {
        let l = layout(3);
        let slider = |i| Focus::Game(i, Part::Slider);
        let cases = [
            (
                Focus::Rest(RestPart::Slider),
                Up,
                Focus::Rest(RestPart::Slider),
            ),
            (Focus::Rest(RestPart::Slider), Down, Focus::AddGame),
            (Focus::Rest(RestPart::Reset), Down, Focus::AddGame),
            (Focus::AddGame, Up, Focus::Rest(RestPart::Slider)),
            (Focus::AddGame, Down, slider(0)),
            (slider(0), Up, Focus::AddGame),
            (slider(0), Down, slider(1)),
            (slider(1), Down, slider(2)),
            (slider(2), Down, slider(2)),
            (slider(2), Up, slider(1)),
            // The part of the row is kept from row to row.
            (
                Focus::Game(0, Part::Remove),
                Down,
                Focus::Game(1, Part::Remove),
            ),
            (
                Focus::Game(2, Part::Toggle),
                Up,
                Focus::Game(1, Part::Toggle),
            ),
            (Focus::Game(0, Part::Toggle), Up, Focus::AddGame),
        ];
        for (from, button, to) in cases {
            let mut state = with_pad(&l);
            state.focus = from;
            assert_eq!(tap(&mut state, &l, button), vec![]);
            assert_eq!(state.focus, to, "{from:?} {button:?}");
        }
    }

    #[test]
    fn with_no_games_down_stops_on_add_a_game() {
        let l = layout(0);
        let mut state = with_pad(&l);
        tap(&mut state, &l, Down);
        assert_eq!(state.focus, Focus::AddGame);
    }

    #[test]
    fn left_and_right_reach_the_slider_the_toggle_and_remove() {
        let l = layout(2);
        let cases = [
            (Part::Slider, Right, Part::Toggle),
            (Part::Toggle, Right, Part::Remove),
            (Part::Remove, Right, Part::Remove),
            (Part::Remove, Left, Part::Toggle),
            (Part::Toggle, Left, Part::Slider),
            (Part::Slider, Left, Part::Slider),
        ];
        for (from, button, to) in cases {
            let mut state = with_pad(&l);
            state.focus = Focus::Game(1, from);
            assert_eq!(tap(&mut state, &l, button), vec![]);
            assert_eq!(state.focus, Focus::Game(1, to), "{from:?} {button:?}");
        }
        let mut state = with_pad(&l);
        state.focus = Focus::Rest(RestPart::Slider);
        tap(&mut state, &l, Right);
        assert_eq!(state.focus, Focus::Rest(RestPart::Reset));
        tap(&mut state, &l, Left);
        assert_eq!(state.focus, Focus::Rest(RestPart::Slider));
    }

    #[test]
    fn a_on_a_slider_starts_changing_it_and_left_right_change_the_value() {
        let l = layout(2);
        let mut state = with_pad(&l);
        assert_eq!(tap(&mut state, &l, A), vec![]);
        assert_eq!(state.adjusting, Some(40));
        assert_eq!(
            tap(&mut state, &l, Right),
            vec![Effect::Percent {
                slider: Slider::Game(0),
                value: 41,
                done: false,
            }]
        );
        assert_eq!(
            tap(&mut state, &l, Left),
            vec![Effect::Percent {
                slider: Slider::Game(0),
                value: 39,
                done: false,
            }]
        );
        // The focus does not move while changing.
        assert_eq!(state.focus, Focus::Game(0, Part::Slider));
    }

    #[test]
    fn holding_a_direction_changes_the_slider_faster() {
        assert_eq!(slider_step(0), 1);
        assert_eq!(slider_step(5), 1);
        assert_eq!(slider_step(6), 2);
        assert_eq!(slider_step(15), 2);
        assert_eq!(slider_step(16), 5);
        assert_eq!(slider_step(400), 5);
        let l = layout(1);
        let mut state = with_pad(&l);
        tap(&mut state, &l, A);
        assert_eq!(
            hold(&mut state, &l, Right, 20),
            vec![Effect::Percent {
                slider: Slider::Game(0),
                value: 45,
                done: false,
            }]
        );
    }

    #[test]
    fn a_slider_stops_at_its_ends() {
        let mut l = layout(1);
        l.games[0].0 = 98;
        let mut state = with_pad(&l);
        tap(&mut state, &l, A);
        assert_eq!(
            hold(&mut state, &l, Right, 30),
            vec![Effect::Percent {
                slider: Slider::Game(0),
                value: 100,
                done: false,
            }]
        );
        l.games[0].0 = -100;
        assert_eq!(
            tap(&mut state, &l, Left),
            vec![Effect::Percent {
                slider: Slider::Game(0),
                value: -100,
                done: false,
            }]
        );
    }

    #[test]
    fn a_finishes_a_change_and_b_puts_the_old_value_back() {
        let mut l = layout(1);
        let mut state = with_pad(&l);
        tap(&mut state, &l, A);
        l.games[0].0 = 55;
        assert_eq!(
            tap(&mut state, &l, A),
            vec![Effect::Percent {
                slider: Slider::Game(0),
                value: 55,
                done: true,
            }]
        );
        assert_eq!(state.adjusting, None);
        tap(&mut state, &l, A);
        assert_eq!(state.adjusting, Some(55));
        l.games[0].0 = 70;
        assert_eq!(
            tap(&mut state, &l, B),
            vec![Effect::Percent {
                slider: Slider::Game(0),
                value: 55,
                done: true,
            }]
        );
        assert_eq!(state.adjusting, None);
    }

    #[test]
    fn moving_away_finishes_a_change_first() {
        let l = layout(2);
        let mut state = with_pad(&l);
        tap(&mut state, &l, A);
        assert_eq!(
            tap(&mut state, &l, Down),
            vec![Effect::Percent {
                slider: Slider::Game(0),
                value: 40,
                done: true,
            }]
        );
        assert_eq!(state.adjusting, None);
        assert_eq!(state.focus, Focus::Game(1, Part::Slider));
    }

    #[test]
    fn the_triggers_change_the_rows_slider_by_one_from_any_part() {
        let l = layout(2);
        for part in [Part::Slider, Part::Toggle, Part::Remove] {
            let mut state = with_pad(&l);
            state.focus = Focus::Game(1, part);
            assert_eq!(
                tap(&mut state, &l, R2),
                vec![Effect::Percent {
                    slider: Slider::Game(1),
                    value: 51,
                    done: false,
                }]
            );
            assert_eq!(
                hold(&mut state, &l, L2, 30),
                vec![Effect::Percent {
                    slider: Slider::Game(1),
                    value: 49,
                    done: false,
                }]
            );
            assert_eq!(state.focus, Focus::Game(1, part));
        }
        let mut state = with_pad(&l);
        state.focus = Focus::Rest(RestPart::Reset);
        assert_eq!(
            tap(&mut state, &l, L2),
            vec![Effect::Percent {
                slider: Slider::Rest,
                value: -1,
                done: false,
            }]
        );
        state.focus = Focus::AddGame;
        assert_eq!(tap(&mut state, &l, R2), vec![]);
    }

    #[test]
    fn x_turns_the_focused_game_on_or_off_from_any_part() {
        let l = layout(2);
        for part in [Part::Slider, Part::Toggle, Part::Remove] {
            let mut state = with_pad(&l);
            state.focus = Focus::Game(1, part);
            assert_eq!(tap(&mut state, &l, X), vec![Effect::ToggleGame(1)]);
            assert_eq!(state.focus, Focus::Game(1, part));
            // Held, it toggles once.
            assert_eq!(hold(&mut state, &l, X, 3), vec![]);
        }
        let mut state = with_pad(&l);
        state.focus = Focus::AddGame;
        assert_eq!(tap(&mut state, &l, X), vec![]);
    }

    #[test]
    fn a_on_the_toggle_and_on_reset_act() {
        let l = layout(1);
        let mut state = with_pad(&l);
        state.focus = Focus::Game(0, Part::Toggle);
        assert_eq!(tap(&mut state, &l, A), vec![Effect::ToggleGame(0)]);
        state.focus = Focus::Rest(RestPart::Reset);
        assert_eq!(
            tap(&mut state, &l, A),
            vec![Effect::Percent {
                slider: Slider::Rest,
                value: 0,
                done: true,
            }]
        );
    }

    #[test]
    fn removing_asks_first_and_starts_on_keep() {
        let l = layout(2);
        let mut state = with_pad(&l);
        state.focus = Focus::Game(1, Part::Remove);
        assert_eq!(tap(&mut state, &l, A), vec![]);
        assert_eq!(state.focus, Focus::Confirm(1, false));
        // A on Keep keeps it.
        assert_eq!(tap(&mut state, &l, A), vec![]);
        assert_eq!(state.focus, Focus::Game(1, Part::Remove));
        // Ask again, move to Remove, and confirm.
        tap(&mut state, &l, A);
        tap(&mut state, &l, Right);
        assert_eq!(state.focus, Focus::Confirm(1, true));
        assert_eq!(tap(&mut state, &l, A), vec![Effect::RemoveGame(1)]);
        state.removed(1, 1);
        assert_eq!(state.focus, Focus::Game(0, Part::Remove));
        state.removed(0, 0);
        assert_eq!(state.focus, Focus::AddGame);
    }

    #[test]
    fn b_and_the_other_buttons_never_remove() {
        let l = layout(1);
        for button in [B, Up, Down, X, Y, L1, R1, L2, R2] {
            let mut state = with_pad(&l);
            state.focus = Focus::Confirm(0, true);
            let effects = tap(&mut state, &l, button);
            assert!(
                !effects.contains(&Effect::RemoveGame(0)),
                "{button:?} removed"
            );
        }
        let mut state = with_pad(&l);
        state.focus = Focus::Confirm(0, true);
        tap(&mut state, &l, B);
        assert_eq!(state.focus, Focus::Game(0, Part::Remove));
        // A held down from the press that asked does not confirm.
        let mut state = with_pad(&l);
        state.focus = Focus::Confirm(0, true);
        assert_eq!(hold(&mut state, &l, A, 2), vec![]);
    }

    #[test]
    fn y_opens_the_list_from_any_page_and_b_closes_it() {
        let l = layout(2);
        for page in [Page::Games, Page::Status, Page::About] {
            let mut state = with_pad(&l);
            state.page = page;
            state.focus = home_focus(page, &l);
            let effects = tap(&mut state, &l, Y);
            assert_eq!(effects.last(), Some(&Effect::OpenPicker));
            assert_eq!(effects.len(), if page == Page::Games { 1 } else { 2 });
            assert_eq!(state.page, Page::Games);
            assert!(state.picker_open);
            assert_eq!(state.focus, Focus::Pick(0));
        }
        let mut state = with_pad(&l);
        tap(&mut state, &l, Y);
        assert_eq!(tap(&mut state, &l, B), vec![Effect::ClosePicker]);
        assert!(!state.picker_open);
        assert_eq!(state.focus, Focus::AddGame);
    }

    #[test]
    fn the_list_moves_by_row_jumps_by_five_and_a_picks() {
        let l = layout(0);
        let mut state = with_pad(&l);
        assert_eq!(tap(&mut state, &l, A), vec![Effect::OpenPicker]);
        assert_eq!(state.focus, Focus::Pick(0));
        tap(&mut state, &l, Up);
        assert_eq!(state.focus, Focus::Pick(0));
        tap(&mut state, &l, Down);
        hold(&mut state, &l, Down, 1);
        assert_eq!(state.focus, Focus::Pick(2));
        tap(&mut state, &l, R1);
        assert_eq!(state.focus, Focus::Pick(7));
        tap(&mut state, &l, R1);
        assert_eq!(state.focus, Focus::Pick(9));
        tap(&mut state, &l, Down);
        assert_eq!(state.focus, Focus::Pick(9));
        tap(&mut state, &l, L1);
        assert_eq!(state.focus, Focus::Pick(4));
        assert_eq!(tap(&mut state, &l, A), vec![Effect::PickRow(4)]);
        // Nothing else leaks out of the open list.
        assert_eq!(tap(&mut state, &l, X), vec![]);
        assert_eq!(tap(&mut state, &l, L2), vec![]);
    }

    #[test]
    fn an_empty_list_cannot_be_picked_from() {
        let mut l = layout(0);
        l.picks = 0;
        let mut state = with_pad(&l);
        tap(&mut state, &l, Y);
        assert_eq!(tap(&mut state, &l, A), vec![]);
        assert_eq!(tap(&mut state, &l, Down), vec![]);
        assert_eq!(state.focus, Focus::Pick(0));
    }

    #[test]
    fn a_pick_lands_on_the_new_rows_slider_ready_to_change() {
        let l = layout(3);
        let mut state = with_pad(&l);
        tap(&mut state, &l, Y);
        state.picked(2, 40);
        assert!(!state.picker_open);
        assert_eq!(state.focus, Focus::Game(2, Part::Slider));
        assert_eq!(state.adjusting, Some(40));
        assert_eq!(
            tap(&mut state, &l, Right),
            vec![Effect::Percent {
                slider: Slider::Game(2),
                value: 61,
                done: false,
            }]
        );
        // Picked by touch: the focus lands there, nothing is being changed.
        let mut state = State::new(&l);
        state.picker_opened_by_touch();
        state.picked(1, 40);
        assert_eq!(state.focus, Focus::Game(1, Part::Slider));
        assert_eq!(state.adjusting, None);
        assert!(!state.highlight);
    }

    #[test]
    fn the_shoulder_buttons_go_round_the_pages() {
        let l = layout(1);
        let mut state = with_pad(&l);
        let mut seen = Vec::new();
        for _ in 0..3 {
            tap(&mut state, &l, R1);
            seen.push(state.page);
        }
        assert_eq!(seen, vec![Page::Status, Page::About, Page::Games]);
        assert_eq!(tap(&mut state, &l, L1), vec![Effect::ShowPage(Page::About)]);
        assert_eq!(state.focus, Focus::Item(0));
        // Held, the page changes once.
        assert_eq!(hold(&mut state, &l, L1, 1), vec![]);
    }

    #[test]
    fn the_status_page_is_a_list() {
        let l = layout(1);
        let mut state = with_pad(&l);
        tap(&mut state, &l, R1);
        assert_eq!(state.focus, Focus::Item(0));
        assert_eq!(tap(&mut state, &l, A), vec![Effect::PressItem(0)]);
        tap(&mut state, &l, Right);
        assert_eq!(state.focus, Focus::Item(1));
        tap(&mut state, &l, Down);
        assert_eq!(tap(&mut state, &l, A), vec![Effect::PressItem(2)]);
        tap(&mut state, &l, Down);
        tap(&mut state, &l, Down);
        assert_eq!(state.focus, Focus::Item(3));
        // The slider there changes like the others.
        tap(&mut state, &l, A);
        assert_eq!(state.adjusting, Some(40));
        assert_eq!(
            tap(&mut state, &l, Left),
            vec![Effect::Percent {
                slider: Slider::NewGame,
                value: 39,
                done: false,
            }]
        );
        tap(&mut state, &l, A);
        assert_eq!(tap(&mut state, &l, B), vec![Effect::ShowPage(Page::Games)]);
        assert_eq!(state.focus, Focus::Game(0, Part::Slider));
    }

    #[test]
    fn a_disabled_item_can_be_focused_but_not_pressed() {
        let mut l = layout(1);
        l.status[1] = Item::Button { enabled: false };
        let mut state = with_pad(&l);
        tap(&mut state, &l, R1);
        tap(&mut state, &l, Down);
        assert_eq!(state.focus, Focus::Item(1));
        assert_eq!(tap(&mut state, &l, A), vec![]);
    }

    #[test]
    fn a_touch_takes_the_focus_and_hides_the_highlight() {
        let l = layout(2);
        let mut state = with_pad(&l);
        tap(&mut state, &l, A);
        state.touched(Focus::Game(1, Part::Toggle));
        assert_eq!(state.focus, Focus::Game(1, Part::Toggle));
        assert!(!state.highlight);
        assert_eq!(state.adjusting, None);
        // The next press shows the highlight where the touch was.
        assert_eq!(tap(&mut state, &l, Down), vec![]);
        assert!(state.highlight);
        assert_eq!(state.focus, Focus::Game(1, Part::Toggle));
        state.page_touched(Page::About, &l);
        assert_eq!((state.page, state.focus), (Page::About, Focus::Item(0)));
    }

    #[test]
    fn the_focus_settles_when_rows_vanish_under_it() {
        let mut l = layout(3);
        let mut state = with_pad(&l);
        state.focus = Focus::Game(2, Part::Toggle);
        l.games.truncate(1);
        state.settle(&l);
        assert_eq!(state.focus, Focus::Game(0, Part::Slider));
        l.games.clear();
        state.focus = Focus::Confirm(0, true);
        state.settle(&l);
        assert_eq!(state.focus, Focus::AddGame);
        state.picker_open = true;
        state.focus = Focus::Pick(9);
        l.picks = 3;
        state.settle(&l);
        assert_eq!(state.focus, Focus::Pick(2));
    }

    #[test]
    fn keys_stand_for_buttons() {
        let cases = [
            ("\u{F700}", Up),
            ("\u{F701}", Down),
            ("\u{F702}", Left),
            ("\u{F703}", Right),
            ("\n", A),
            (" ", A),
            ("\u{1b}", B),
            ("\u{8}", B),
            ("x", X),
            ("Y", Y),
            ("\u{F72C}", L1),
            ("\u{F72D}", R1),
            ("q", L1),
            ("e", R1),
            ("-", L2),
            ("=", R2),
        ];
        for (text, button) in cases {
            assert_eq!(button_for_key(text), Some(button), "{text:?}");
        }
        for text in ["a", "\t", "", "1", "\u{F704}"] {
            assert_eq!(button_for_key(text), None, "{text:?}");
        }
    }

    fn bar(state: &State, layout: &Layout) -> Vec<String> {
        prompts(state, layout)
            .into_iter()
            .map(|p| format!("{} {}", p.glyphs.join("+"), p.label))
            .collect()
    }

    #[test]
    fn the_prompt_bar_follows_the_focus() {
        let mut l = layout(2);
        let mut state = with_pad(&l);
        assert_eq!(
            bar(&state, &l),
            [
                "A Change",
                "X Turn off",
                "L2+R2 Change",
                "Y Add a game",
                "L1+R1 Pages"
            ]
        );
        l.games[0].1 = false;
        state.focus = Focus::Game(0, Part::Toggle);
        assert_eq!(
            bar(&state, &l),
            ["A Turn on", "L2+R2 Change", "Y Add a game", "L1+R1 Pages"]
        );
        state.focus = Focus::Game(0, Part::Remove);
        assert_eq!(bar(&state, &l)[..2], ["A Remove", "X Turn on"]);
        state.focus = Focus::Confirm(0, false);
        assert_eq!(bar(&state, &l), ["A Choose", "B Keep"]);
        state.focus = Focus::AddGame;
        assert_eq!(bar(&state, &l), ["A Add a game", "L1+R1 Pages"]);
        state.focus = Focus::Rest(RestPart::Reset);
        assert_eq!(bar(&state, &l)[0], "A Back to 0%");
        state.focus = Focus::Game(1, Part::Slider);
        state.adjusting = Some(50);
        assert_eq!(
            bar(&state, &l),
            ["\u{25c0}+\u{25b6} Change", "L2+R2 By 1", "A Done", "B Undo"]
        );
    }

    #[test]
    fn the_prompt_bar_for_the_list_the_other_pages_and_touch() {
        let l = layout(2);
        let mut state = with_pad(&l);
        state.picker_open = true;
        state.focus = Focus::Pick(0);
        assert_eq!(bar(&state, &l), ["A Pick", "B Close", "L1+R1 Jump"]);
        state.picker_open = false;
        state.page = Page::Status;
        state.focus = Focus::Item(0);
        assert_eq!(bar(&state, &l), ["A Select", "B Games", "L1+R1 Pages"]);
        state.focus = Focus::Item(2);
        assert_eq!(bar(&state, &l)[0], "A Switch");
        state.focus = Focus::Item(3);
        assert_eq!(bar(&state, &l)[..2], ["A Change", "L2+R2 Change"]);
        // With touch in use nothing claims a focus the user cannot see.
        let touch = State::new(&l);
        assert_eq!(bar(&touch, &l), ["Y Add a game", "L1+R1 Pages"]);
    }
}
