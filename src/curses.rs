// An abstract layer towards ncurses-rs, which provides keycode, color scheme support
// Modeled after fzf

use ncurses::*;
use getopts;
use std::sync::RwLock;
use std::collections::HashMap;
use libc::{STDIN_FILENO, STDERR_FILENO, fdopen, c_char};
use std::io::{stdout, stdin, Read, Write};
use std::cmp::min;

//use std::io::Write;
macro_rules! println_stderr(
    ($($arg:tt)*) => { {
        let r = writeln!(&mut ::std::io::stderr(), $($arg)*);
        r.expect("failed printing to stderr");
    } }
);

pub static COLOR_NORMAL:        i16 = 0;
pub static COLOR_PROMPT:        i16 = 1;
pub static COLOR_MATCHED:       i16 = 2;
pub static COLOR_CURRENT:       i16 = 3;
pub static COLOR_CURRENT_MATCH: i16 = 4;
pub static COLOR_SPINNER:       i16 = 5;
pub static COLOR_INFO:          i16 = 6;
pub static COLOR_CURSOR:        i16 = 7;
pub static COLOR_SELECTED:      i16 = 8;
pub static COLOR_HEADER:        i16 = 9;
static COLOR_USER:              i16 = 10;

lazy_static! {
    static ref COLOR_MAP: RwLock<HashMap<i16, attr_t>> = RwLock::new(HashMap::new());
    static ref FG: RwLock<i16> = RwLock::new(7);
    static ref BG: RwLock<i16> = RwLock::new(0);
    static ref USE_COLOR: RwLock<bool> = RwLock::new(true);
}

pub fn init(theme: Option<&ColorTheme>, is_black: bool, _use_mouse: bool) {
    // initialize ncurses
    let mut use_color = USE_COLOR.write().unwrap();

    if let Some(theme) = theme {
        let base_theme = if tigetnum("colors") >= 256 {
            DARK256
        } else {
            DEFAULT16
        };

        init_pairs(&base_theme, theme, is_black);
        *use_color = true;
    } else {
        *use_color = false;
    }
}

fn init_pairs(base: &ColorTheme, theme: &ColorTheme, is_black: bool) {
    let mut fg = FG.write().unwrap();
    let mut bg = BG.write().unwrap();


    *fg = shadow(base.fg, theme.fg);
    *bg = shadow(base.bg, theme.bg);

    if is_black {
        *bg = COLOR_BLACK;
    } else if theme.use_default {
        *fg = COLOR_DEFAULT;
        *bg = COLOR_DEFAULT;
        use_default_colors();
    }

    if !theme.use_default {
        assume_default_colors(shadow(base.fg, theme.fg) as i32, shadow(base.bg, theme.bg) as i32);
    }

    start_color();

    init_pair(COLOR_PROMPT,        shadow(base.prompt,        theme.prompt),        *bg);
    init_pair(COLOR_MATCHED,       shadow(base.matched,       theme.matched),       shadow(base.matched_bg, theme.matched_bg));
    init_pair(COLOR_CURRENT,       shadow(base.current,       theme.current),       shadow(base.current_bg, theme.current_bg));
    init_pair(COLOR_CURRENT_MATCH, shadow(base.current_match, theme.current_match), shadow(base.current_match_bg, theme.current_match_bg));
    init_pair(COLOR_SPINNER,       shadow(base.spinner,       theme.spinner),       *bg);
    init_pair(COLOR_INFO,          shadow(base.info,          theme.info),          *bg);
    init_pair(COLOR_CURSOR,        shadow(base.cursor,        theme.cursor),        shadow(base.current_bg, theme.current_bg));
    init_pair(COLOR_SELECTED,      shadow(base.selected,      theme.selected),      shadow(base.current_bg, theme.current_bg));
    init_pair(COLOR_HEADER,        shadow(base.header,        theme.header),        shadow(base.bg, theme.bg));
}


pub fn get_color_pair(fg: i16, bg: i16) -> attr_t {
    let fg = if fg == -1 { *FG.read().unwrap() } else {fg};
    let bg = if bg == -1 { *BG.read().unwrap() } else {bg};

    let key = (fg << 8) + bg;
    let mut color_map = COLOR_MAP.write().unwrap();
    let pair_num = color_map.len() as i16;
    let pair = color_map.entry(key).or_insert_with(|| {
        let next_pair = COLOR_USER + pair_num;
        init_pair(next_pair, fg, bg);
        COLOR_PAIR(next_pair)
    });
    *pair
}

#[derive(PartialEq, Eq, Clone, Debug, Copy)]
pub enum Margin {
    Fixed(i32),
    Percent(i32),
}

// A curse object is an abstraction of the screen to be draw on
// |
// |
// |
// +------------+ start_line
// |  ^         |
// | <          | <-- top = start_line + margin_top
// |  (margins) |
// |           >| <-- bottom = end_line - margin_bottom
// |          v |
// +------------+ end_line
// |
// |

struct Screen(SCREEN);

impl Screen {
    pub fn getyx(&self) -> (i32, i32) {
        let mut y = 0;
        let mut x = 0;
        getyx(self.0, &mut y, &mut x);
        (y, x)
    }

    pub fn getmaxyx(&self) -> (i32, i32) {
        let mut max_y = 0;
        let mut max_x = 0;
        getmaxyx(self.0, &mut max_y, &mut max_x);
        (max_y, max_x)
    }

    pub fn clrtoeol(&self) {
        clrtoeol();
    }

    pub fn endwin(&self) {
        endwin();
    }

    pub fn refresh(&self) {
        refresh();
    }

    pub fn mv(&self, y: i32, x: i32) {
        mv(y, x);
    }
}

impl Drop for Screen {
    fn drop(&mut self) {
        //delscreen(self.0);
    }
}

pub struct Curses {
    screen: Screen,
    top: i32,
    bottom: i32,
    left: i32,
    right: i32,
    height: Margin,
    start_y: i32,
    margin_top: Margin,
    margin_bottom: Margin,
    margin_left: Margin,
    margin_right: Margin,
}

unsafe impl Send for Curses {}

impl Curses {
    pub fn new(options: &getopts::Matches) -> Self {
        let local_conf = LcCategory::all;
        setlocale(local_conf, "en_US.UTF-8"); // for showing wide characters


        let margins = if let Some(margin_option) = options.opt_str("margin") {
            Curses::parse_margin(&margin_option)
        } else {
            (Margin::Fixed(0), Margin::Fixed(0), Margin::Fixed(0), Margin::Fixed(0))
        };
        let (margin_top, margin_right, margin_bottom, margin_left) = margins;

        let height = if let Some(height_option) = options.opt_str("height") {
            Curses::parse_margin_string(&height_option)
        } else {
            Margin::Percent(100)
        };

        //let stdin = unsafe { fdopen(STDIN_FILENO, "r".as_ptr() as *const c_char)};
        //let stderr = unsafe { fdopen(STDERR_FILENO, "w".as_ptr() as *const c_char)};
        //let screen = newterm(None, stderr, stdin);
        //set_term(screen);

        let s = initscr();
        refresh();
        raw();
        noecho();

        let screen = Screen(s);

        match height {
            Margin::Percent(100) => {}
            _ => {
                putp(&tigetstr("rmcup"));
                refresh();
            }
        };

        let (y, x) = Curses::get_cursor_pos();
        let (max_y, max_x) = screen.getmaxyx();
        Curses::reserve_lines(&screen, max_y, height);

        let start_y = match height {
            Margin::Percent(100) => 0,
            Margin::Percent(p) => min(y, max_y- p*max_y/100),
            Margin::Fixed(rows) => min(y, max_y - rows),
        };

        debug!("curses: height = {:?}, y/x: {}/{}, max: {}/{}, start_y: {}", height, y, x, max_y, max_x, start_y);

        let mut curses = Curses {
            screen: screen,
            top: 0,
            bottom: 0,
            left: 0,
            right: 0,
            height,
            start_y,
            margin_top,
            margin_bottom,
            margin_left,
            margin_right,
        };
        curses.resize();
        curses
    }

    fn parse_margin_string(margin: &str) -> Margin {
        if margin.ends_with("%") {
            Margin::Percent(margin[0..margin.len()-1].parse::<i32>().unwrap_or(100))
        } else {
            Margin::Fixed(margin.parse::<i32>().unwrap_or(0))
        }
    }

    fn parse_margin(margin : &str) -> (Margin, Margin, Margin, Margin) {
        let margins = margin.split(",").collect::<Vec<&str>>();

        match margins.len() {
            1 => {
                let margin = Curses::parse_margin_string(margins[0]);
                (margin, margin, margin, margin)
            }
            2 => {
                let margin_tb = Curses::parse_margin_string(margins[0]);
                let margin_rl = Curses::parse_margin_string(margins[1]);
                (margin_tb, margin_rl, margin_tb, margin_rl)
            }
            3 => {
                let margin_top = Curses::parse_margin_string(margins[0]);
                let margin_rl = Curses::parse_margin_string(margins[1]);
                let margin_bottom = Curses::parse_margin_string(margins[2]);
                (margin_top, margin_rl, margin_bottom, margin_rl)
            }
            4 => {
                let margin_top = Curses::parse_margin_string(margins[0]);
                let margin_right = Curses::parse_margin_string(margins[1]);
                let margin_bottom = Curses::parse_margin_string(margins[2]);
                let margin_left = Curses::parse_margin_string(margins[3]);
                (margin_top, margin_right, margin_bottom, margin_left)
            }
            _ => (Margin::Fixed(0), Margin::Fixed(0), Margin::Fixed(0), Margin::Fixed(0))
        }
    }

    fn get_color(&self, pair: i16, is_bold: bool) -> attr_t {
        if *USE_COLOR.read().unwrap() {
            attr_color(pair, is_bold)
        } else {
            attr_mono(pair, is_bold)
        }
    }

    fn get_cursor_pos() -> (i32, i32) {
        let mut stdout = stdout();
        let mut f = stdin();
        putp("\x1B[6n");
        refresh();

        let mut read_chars = Vec::new();
        loop {
            let mut buf = [0; 1];
            let _ = f.read(&mut buf);
            read_chars.push(buf[0]);
            if buf[0] == b'R' {
                break;
            }
        }
        let s = String::from_utf8(read_chars).unwrap();
        let t: Vec<&str> = s[2..s.len()-1].split(';').collect();
        (t[0].parse::<i32>().unwrap() - 1, t[1].parse::<i32>().unwrap() - 1)
    }

    fn reserve_lines(screen: &Screen, max_y: i32, height: Margin) {
        let rows = match height {
            Margin::Percent(100) => {return;}
            Margin::Percent(percent) => max_y*percent/100,
            Margin::Fixed(rows) => rows,
        };

        debug!("curses:reserve_lines: max_y {}, rows: {}", max_y, rows);

        for i in 0..(rows-1) {
            mv(i, 0);
            printw(" "); // something other than space is necessary for "exit_ca_mode"
        }
        printw(">"); // something other than space is necessary for "exit_ca_mode"
        refresh();
    }

    pub fn resize(&mut self) {
        let (_, max_x) = self.screen.getmaxyx();

        let height = self.height_in_rows();

        self.top = self.start_y + match self.margin_top {
            Margin::Fixed(num) => num,
            Margin::Percent(per) => per * height / 100,
        };

        self.bottom = self.start_y + height - match self.margin_bottom {
            Margin::Fixed(num) => num,
            Margin::Percent(per) => per * height / 100,
        };

        self.left = match self.margin_left {
            Margin::Fixed(num) => num,
            Margin::Percent(per) => per * max_x / 100,
        };

        self.right = max_x - match self.margin_right {
            Margin::Fixed(num) => num,
            Margin::Percent(per) => per * max_x / 100,
        };

        debug!("curses:resize: after, trbl: {}, {}, {}, {}", self.top, self.right, self.bottom, self.left);
    }

    fn height_in_rows(&self) -> i32 {
        let (max_y, _) = self.screen.getmaxyx();
        match self.height {
            Margin::Percent(100) => max_y,
            Margin::Percent(p) => min(max_y, p*max_y/100),
            Margin::Fixed(rows) => min(max_y, rows),
        }
    }

    pub fn mv(&self, y: i32, x: i32) {
        self.screen.mv(y+self.top, x+self.left);
        let (yy, xx) = self.screen.getyx();
        debug!("curses:mv({}, {}); after: {}, {}, {}/{}", y, x, y + self.top, x + self.left, yy, xx);
    }

    pub fn get_maxyx(&self) -> (i32, i32) {
        let (a, b) = (self.bottom-self.top, self.right-self.left);
        debug!("get_maxyx: {}/{}, trbl: {}, {}, {}, {}", a, b, self.top, self.right, self.bottom, self.left);
        (a, b)
    }

    pub fn getyx(&self) -> (i32, i32) {
        let (y, x) = self.screen.getyx();
        (y-self.top, x-self.left)
    }

    pub fn clrtoeol(&self) {
        debug!("curses:clrtoeol();");
        //self.screen.clrtoeol();
        let spaces = " ".repeat((self.right - self.bottom) as usize);
        let (y, x) = self.screen.getyx();
        self.screen.mv(y, 0);
        printw(&spaces);
        self.screen.mv(y, x);
    }

    pub fn erase(&self) {
        debug!("curses:erase(); top: {}, bottom:{}", self.top, self.bottom);
        //self.screen.erase();
        let spaces = " ".repeat((self.right - self.bottom) as usize);
        for i in self.top..self.bottom {
            self.screen.mv(i, 0);
            printw(&spaces);
            //self.screen.clrtoeol();
        }
    }

    pub fn cprint(&self, text: &str, pair: i16, is_bold: bool) {
        debug!("curses:addstr({:?});", text);
        let attr = self.get_color(pair, is_bold);
        attron(attr);
        addstr(text);
        attroff(attr);
    }

    pub fn caddch(&self, ch: char, pair: i16, is_bold: bool) {
        debug!("curses:addstr(&{:?}.to_string());", ch);
        let attr = self.get_color(pair, is_bold);
        attron(attr);
        addstr(&ch.to_string()); // to support wide character
        attroff(attr);
    }

    pub fn printw(&self, text: &str) {
        debug!("curses:printw({:?});", text);
        printw(text);
    }

    pub fn close(&self) {
        debug!("curses:close();");
        self.erase();
        self.mv(0, 0);
        self.screen.refresh();
        if self.height != Margin::Percent(100) {
            putp(&tigetstr("smcup"));
            refresh();
        }
        endwin();
        delscreen(self.screen.0);
    }

    pub fn attr_on(&self, attr: attr_t) {
        if attr == 0 {
            attrset(0);
        } else {
            attron(attr);
        }
    }

    pub fn refresh(&self) {
        debug!("curses:refresh();");
        self.screen.refresh();
    }
}

// use default if x is COLOR_UNDEFINED, else use x
fn shadow(default: i16, x: i16) -> i16 {
    if x == COLOR_UNDEFINED { default } else { x }
}


fn attr_color(pair: i16, is_bold: bool) -> attr_t {
    let attr = if pair > COLOR_NORMAL {COLOR_PAIR(pair)} else {0};

    attr | if is_bold {A_BOLD()} else {0}
}

fn attr_mono(pair: i16, is_bold: bool) -> attr_t {
    let mut attr = 0;
    match pair {
        x if x == COLOR_NORMAL => {
            if is_bold {
                attr = A_REVERSE();
            }
        }
        x if x == COLOR_MATCHED => {
            attr = A_UNDERLINE();
        }
        x if x == COLOR_CURRENT_MATCH => {
            attr = A_UNDERLINE() | A_REVERSE()
        }
        _ => {}
    }
    attr | if is_bold {A_BOLD()} else {0}
}

const COLOR_DEFAULT: i16 = -1;
const COLOR_UNDEFINED: i16 = -2;

#[derive(Clone, Debug)]
pub struct ColorTheme {
    use_default: bool,

    fg: i16, // text fg
    bg: i16, // text bg
    matched: i16,
    matched_bg: i16,
    current: i16,
    current_bg: i16,
    current_match: i16,
    current_match_bg: i16,
    spinner: i16,
    info: i16,
    prompt: i16,
    cursor: i16,
    selected: i16,
    header: i16,
}

impl ColorTheme {
    pub fn new() -> Self {
        ColorTheme {
            use_default:  true,
            fg:               COLOR_UNDEFINED,
            bg:               COLOR_UNDEFINED,
            matched:          COLOR_UNDEFINED,
            matched_bg:       COLOR_UNDEFINED,
            current:          COLOR_UNDEFINED,
            current_bg:       COLOR_UNDEFINED,
            current_match:    COLOR_UNDEFINED,
            current_match_bg: COLOR_UNDEFINED,
            spinner:          COLOR_UNDEFINED,
            info:             COLOR_UNDEFINED,
            prompt:           COLOR_UNDEFINED,
            cursor:           COLOR_UNDEFINED,
            selected:         COLOR_UNDEFINED,
            header:           COLOR_UNDEFINED,
        }
    }

    pub fn from_options(color: &str) -> Self {
        let mut theme = ColorTheme::new();
        for pair in color.split(',') {
            let color: Vec<&str> = pair.split(':').collect();
            if color.len() < 2 {
                theme = match color[0] {
                    "molokai" => MONOKAI256.clone(),
                    "light" => LIGHT256.clone(),
                    "16"  => DEFAULT16.clone(),
                    "dark" | _ => DARK256.clone(),
                }
            }

            match color[0] {
                "fg"               => theme.fg = color[1].parse().unwrap_or(COLOR_UNDEFINED),
                "bg"               => theme.bg = color[1].parse().unwrap_or(COLOR_UNDEFINED),
                "matched"          => theme.matched = color[1].parse().unwrap_or(COLOR_UNDEFINED),
                "matched_bg"       => theme.matched_bg = color[1].parse().unwrap_or(COLOR_UNDEFINED),
                "current"          => theme.current = color[1].parse().unwrap_or(COLOR_UNDEFINED),
                "current_bg"       => theme.current_bg = color[1].parse().unwrap_or(COLOR_UNDEFINED),
                "current_match"    => theme.current_match = color[1].parse().unwrap_or(COLOR_UNDEFINED),
                "current_match_bg" => theme.current_match_bg = color[1].parse().unwrap_or(COLOR_UNDEFINED),
                "spinner"          => theme.spinner = color[1].parse().unwrap_or(COLOR_UNDEFINED),
                "info"             => theme.info = color[1].parse().unwrap_or(COLOR_UNDEFINED),
                "prompt"           => theme.prompt = color[1].parse().unwrap_or(COLOR_UNDEFINED),
                "cursor"           => theme.cursor = color[1].parse().unwrap_or(COLOR_UNDEFINED),
                "selected"         => theme.selected = color[1].parse().unwrap_or(COLOR_UNDEFINED),
                "header"           => theme.header = color[1].parse().unwrap_or(COLOR_UNDEFINED),
                _ => {}
            }
        }
        theme
    }
}

const DEFAULT16: ColorTheme = ColorTheme {
    use_default:   true,
    fg:               15,
    bg:               0,
    matched:          COLOR_GREEN,
    matched_bg:       COLOR_BLACK,
    current:          COLOR_YELLOW,
    current_bg:       COLOR_BLACK,
    current_match:    COLOR_GREEN,
    current_match_bg: COLOR_BLACK,
    spinner:          COLOR_GREEN,
    info:             COLOR_WHITE,
    prompt:           COLOR_BLUE,
    cursor:           COLOR_RED,
    selected:         COLOR_MAGENTA,
    header:           COLOR_CYAN,
};

const DARK256: ColorTheme = ColorTheme {
    use_default:   true,
    fg:               15,
    bg:               0,
    matched:          108,
    matched_bg:       0,
    current:          254,
    current_bg:       236,
    current_match:    151,
    current_match_bg: 236,
    spinner:          148,
    info:             144,
    prompt:           110,
    cursor:           161,
    selected:         168,
    header:           109,
};

const MONOKAI256: ColorTheme = ColorTheme {
    use_default:   true,
    fg:               252,
    bg:               234,
    matched:          234,
    matched_bg:       186,
    current:          254,
    current_bg:       236,
    current_match:    234,
    current_match_bg: 186,
    spinner:          148,
    info:             144,
    prompt:           110,
    cursor:           161,
    selected:         168,
    header:           109,
};

const LIGHT256: ColorTheme = ColorTheme {
    use_default:   true,
    fg:               15,
    bg:               0,
    matched:          0,
    matched_bg:       220,
    current:          237,
    current_bg:       251,
    current_match:    66,
    current_match_bg: 251,
    spinner:          65,
    info:             101,
    prompt:           25,
    cursor:           161,
    selected:         168,
    header:           31,
};
