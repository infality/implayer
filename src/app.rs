use __core::time::Duration;
use souvlaki::{MediaControlEvent, MediaControls, MediaPlayback, PlatformConfig};
use std::{
    cmp::Ordering,
    collections::{hash_map::DefaultHasher, VecDeque},
    env, ffi,
    fs::{self},
    hash::{Hash, Hasher},
    path::{Path, PathBuf},
    process::Child,
    sync::{
        mpsc::{self, Receiver, Sender},
        Arc, Mutex,
    },
    thread::{self, JoinHandle},
    time::Instant,
};

use crate::player;
use crate::util;
use crate::{actions, download};
use imgui::{internal::DataTypeKind, *};

// TODO Context menu padding not working for first level menu, missing bindings to do smth like https://github.com/ocornut/imgui/issues/4129#issuecomment-916195585

pub const TEXT1: [f32; 4] = [0.80, 0.80, 0.80, 1.0];
pub const TEXT2: [f32; 4] = [0.50, 0.50, 0.50, 1.0];
pub const DARK1: [f32; 4] = [0.07, 0.07, 0.07, 1.0];
pub const DARK2: [f32; 4] = [0.11, 0.11, 0.11, 1.0];
pub const DARK3: [f32; 4] = [0.13, 0.13, 0.13, 1.0];
pub const DARK4: [f32; 4] = [0.14, 0.14, 0.14, 1.0];
pub const DARK5: [f32; 4] = [0.15, 0.15, 0.15, 1.0];
pub const DARK6: [f32; 4] = [0.17, 0.17, 0.17, 1.0];
pub const DARK7: [f32; 4] = [0.20, 0.20, 0.20, 1.0];
pub const PRIMARY1: [f32; 4] = [0.00, 0.28, 0.50, 1.0];
pub const PRIMARY2: [f32; 4] = [0.00, 0.34, 0.61, 1.0];
pub const PLAYING_COLOR: [f32; 4] = [0.00, 0.70, 0.00, 1.0];
pub const NOT_EXISTING_COLOR: [f32; 4] = [0.70, 0.00, 0.00, 1.0];
pub const HOVERED_BG: [f32; 4] = DARK5;
pub const ACTIVE_BG: [f32; 4] = DARK6;
pub const DRAG: [f32; 4] = [0.00, 0.28, 0.50, 0.85];
pub const TRANSPARENT: [f32; 4] = [0.0, 0.0, 0.0, 0.0];
pub const INFO: [f32; 4] = [0.0, 0.2, 0.4, 1.0];
pub const ERROR: [f32; 4] = [0.4, 0.0, 0.0, 1.0];
pub const PROGRESS: [f32; 4] = INFO;

pub const ALL_PLAYLIST_NAME: &str = "All";
pub const ALL_UNUSED_PLAYLIST_NAME: &str = "All Unused";
const NEW_PLAYLIST_TEXT: &str = "New playlist name";
const SONG_SEARCH_TEXT: &str = "Song search";

const CONTROLS_HEIGHT: f32 = 100.0;
const TEXTBOXES_HEIGHT: f32 = 24.0;
const SONGS_HEADER_HEIGHT: f32 = 30.0;

const DIRECTORY_COLOR: [f32; 4] = TEXT2;
const PLAYLIST_LIST_BG: [f32; 4] = DARK1;
const SONGS_HEADER_BG: [f32; 4] = DARK1;
const SONG_LIST_BG1: [f32; 4] = DARK2;
const SONG_LIST_BG2: [f32; 4] = DARK1;
const CONTROLS_BG: [f32; 4] = DARK1;

pub struct Playlist {
    pub name: String,
    pub songs: Vec<Song>,
    pub original_hash: u64,
}

impl Playlist {
    pub fn new(name: String, songs: Vec<Song>) -> Playlist {
        let mut hasher = DefaultHasher::new();
        for song in songs.iter() {
            song.hash(&mut hasher);
        }

        Playlist {
            name,
            songs,
            original_hash: hasher.finish(),
        }
    }
}

#[derive(Clone)]
pub struct Song {
    pub path: String,
    pub name: String,
    pub artist: String,
    /// Milliseconds
    pub duration: Option<u64>,
    pub exists: bool,
}

impl Song {
    pub fn new(path: PathBuf, base_path: &str, duration: Option<u64>) -> Song {
        let file_name = path.file_stem().unwrap().to_string_lossy();
        let name_info: Vec<&str> = file_name.splitn(2, " - ").collect();

        Song {
            path: path
                .strip_prefix(base_path)
                .unwrap()
                .to_string_lossy()
                .to_string(),
            name: if name_info.len() > 1 {
                name_info[1].trim().to_string()
            } else {
                String::new()
            },
            artist: name_info[0].trim().to_string(),
            duration,
            exists: path.exists(),
        }
    }

    pub fn is_matching(&self, search_text: &str) -> bool {
        let search_text = search_text.to_lowercase();
        self.name.to_lowercase().contains(&search_text)
            || self.artist.to_lowercase().contains(&search_text)
    }
}

impl Hash for Song {
    fn hash<H: Hasher>(&self, state: &mut H) {
        self.path.hash(state);
        self.duration.hash(state);
    }
}

pub enum SortDirection {
    Ascending,
    Descending,
}
impl SortDirection {
    pub fn get_sort_icon(&self) -> &str {
        match self {
            SortDirection::Ascending => "▲",
            SortDirection::Descending => "▼",
        }
    }
    pub fn get_sort_icon_width(ui: &Ui) -> f32 {
        ui.calc_text_size("▲")[0]
    }
    pub fn apply_direction(&self, ord: Ordering) -> Ordering {
        match self {
            SortDirection::Ascending => ord,
            SortDirection::Descending => ord.reverse(),
        }
    }
}

pub enum SortType {
    Song(SortDirection),
    Artist(SortDirection),
    Duration(SortDirection),
}
impl SortType {
    pub fn compare(&self, a: &Song, b: &Song) -> Ordering {
        match self {
            SortType::Song(dir) => {
                dir.apply_direction(a.name.to_lowercase().cmp(&b.name.to_lowercase()))
            }
            SortType::Artist(dir) => {
                dir.apply_direction(a.artist.to_lowercase().cmp(&b.artist.to_lowercase()))
            }
            SortType::Duration(dir) => dir.apply_direction(a.duration.cmp(&b.duration)),
        }
    }
}

pub enum DownloadState {
    None,
    Downloading(
        Child,
        Receiver<String>,
        Sender<()>,
        Receiver<String>,
        Sender<()>,
    ),
    Postprocessing(
        Child,
        Receiver<String>,
        Sender<()>,
        Receiver<String>,
        Sender<()>,
    ),
}

#[derive(Debug)]
pub enum StatusType {
    Info,
    //Warning,
    Error,
    Progress,
}

impl StatusType {
    fn get_color(&self) -> [f32; 4] {
        match self {
            StatusType::Info => INFO,
            StatusType::Error => ERROR,
            StatusType::Progress => PROGRESS,
        }
    }
}

#[derive(Debug)]
pub struct Status {
    pub info: String,
    pub timestamp: Instant,
    pub r#type: StatusType,
}

pub struct ScrollInfo {
    pub is_scrolling: bool,
    pub scroll_start_time: Instant,
    pub scroll_duration: Duration,
    pub scroll_target_y: f32,
}

pub struct State {
    pub base_path: String,
    pub playlists: Vec<Playlist>,
    pub selected_playlist_index: usize,
    pub selected_song_indices: Vec<usize>,
    pub new_playlist_text: String,
    pub song_search_text: String,
    pub has_textbox_focus: bool,
    pub sort_type: Option<SortType>,

    pub dragged_songs: Vec<Song>,

    pub original_file_name: String,
    pub file_name_text: String,

    pub download_text: String,
    pub download_playlist_index: Option<usize>,
    pub download_path: Option<String>,
    pub download_state: DownloadState,
    pub last_download_status: Option<Instant>,

    pub status_queue: VecDeque<Status>,

    pub playing_playlist_index: Option<usize>,
    pub playing_song_index: Option<usize>,

    pub is_playing: bool,
    pub volume: f32,
    pub player_thread: JoinHandle<()>,
    pub action_tx: Sender<player::PlayerAction>,
    pub song_ended_rx: Receiver<()>,
    pub last_progress: Option<f64>,
    pub position: Arc<Mutex<u64>>,
    pub media_controls: MediaControls,
    pub media_controls_rx: Receiver<MediaControlEvent>,

    pub playlists_scroll_info: ScrollInfo,
    pub songs_scroll_info: ScrollInfo,
    pub add_to_menu_scroll_info: ScrollInfo,
}

impl State {
    pub fn sort_playlists(&mut self) {
        self.playlists.sort_by(|a, b| {
            if a.name == ALL_PLAYLIST_NAME {
                return Ordering::Less;
            }

            // Sort unused playlist below all playlist
            if a.name == ALL_UNUSED_PLAYLIST_NAME && b.name != ALL_PLAYLIST_NAME {
                return Ordering::Less;
            }
            if a.name != ALL_PLAYLIST_NAME && b.name == ALL_UNUSED_PLAYLIST_NAME {
                return Ordering::Greater;
            }

            // Sort directory playlists below everything else
            if a.name.contains('.') && !b.name.contains('.') {
                return Ordering::Greater;
            }
            if !a.name.contains('.') && b.name.contains('.') {
                return Ordering::Less;
            }
            a.name.to_lowercase().cmp(&b.name.to_lowercase())
        });
    }
}

pub fn initialize(hwnd: Option<*mut ffi::c_void>) -> State {
    let args: Vec<String> = env::args().collect();

    let base_path = if args.len() >= 2 {
        if fs::metadata(&args[1]).is_err() || !fs::metadata(&args[1]).unwrap().is_dir() {
            println!("Please pass a directory");
            std::process::exit(1);
        }
        args[1].clone()
    } else {
        let mut exe = env::current_exe().expect("Could not get current directory");
        exe.pop();
        exe.to_string_lossy().to_string()
    };

    let music_extensions = vec!["flac", "mp3", "m4a", "ogg", "wav"];

    let (action_tx, action_rx) = mpsc::channel();
    let (song_ended_tx, song_ended_rx) = mpsc::channel();
    let position = Arc::new(Mutex::new(0));
    let thread_position = position.clone();

    let player_thread = thread::spawn(|| player::run(action_rx, song_ended_tx, thread_position));

    let config = PlatformConfig {
        dbus_name: "ImPlayer",
        display_name: "ImPlayer",
        hwnd,
    };
    let mut media_controls = MediaControls::new(config).unwrap();
    let (media_controls_tx, media_controls_rx) = mpsc::sync_channel(32);
    media_controls
        .attach(move |e| media_controls_tx.send(e).unwrap())
        .unwrap();

    let mut state = State {
        base_path,
        playlists: Vec::new(),
        selected_playlist_index: 0,
        selected_song_indices: Vec::new(),
        new_playlist_text: String::new(),
        song_search_text: String::new(),
        has_textbox_focus: false,
        sort_type: None,

        dragged_songs: Vec::new(),

        original_file_name: String::new(),
        file_name_text: String::new(),

        download_text: String::new(),
        download_playlist_index: None,
        download_path: None,
        download_state: DownloadState::None,
        last_download_status: None,

        status_queue: VecDeque::new(),

        playing_playlist_index: None,
        playing_song_index: None,

        is_playing: false,
        volume: 0.93,
        player_thread,
        action_tx,
        song_ended_rx,
        last_progress: None,
        position,
        media_controls,
        media_controls_rx,

        playlists_scroll_info: ScrollInfo {
            is_scrolling: false,
            scroll_start_time: Instant::now(),
            scroll_duration: Duration::from_millis(200),
            scroll_target_y: 0.0,
        },
        songs_scroll_info: ScrollInfo {
            is_scrolling: false,
            scroll_start_time: Instant::now(),
            scroll_duration: Duration::from_millis(200),
            scroll_target_y: 0.0,
        },
        add_to_menu_scroll_info: ScrollInfo {
            is_scrolling: false,
            scroll_start_time: Instant::now(),
            scroll_duration: Duration::from_millis(200),
            scroll_target_y: 0.0,
        },
    };

    // Parse songs
    let mut songs = Vec::new();
    for file in fs::read_dir(&state.base_path).unwrap().filter(|x| {
        x.as_ref().unwrap().file_type().unwrap().is_file()
            && music_extensions.contains(
                &x.as_ref()
                    .unwrap()
                    .path()
                    .extension()
                    .map_or("", |e| e.to_str().unwrap_or("")),
            )
    }) {
        let path = file.as_ref().unwrap().path();
        songs.push(Song::new(path, &state.base_path, None));
    }

    // Parse playlists
    for file in fs::read_dir(&state.base_path).unwrap().filter(|x| {
        x.as_ref().unwrap().file_type().unwrap().is_file()
            && x.as_ref().unwrap().path().extension() == Some(ffi::OsStr::new("m3u"))
    }) {
        let playlist_name = file
            .as_ref()
            .unwrap()
            .path()
            .file_stem()
            .unwrap()
            .to_string_lossy()
            .to_string();
        let content = fs::read_to_string(file.as_ref().unwrap().path()).unwrap();
        let lines: Vec<&str> = content.lines().collect();
        let mut playlist_songs = Vec::<Song>::new();
        for line in (1..lines.len()).step_by(2) {
            let info = lines[line]
                .split_once(":")
                .unwrap()
                .1
                .split_once(",")
                .unwrap();
            let duration = info.0.parse::<u64>().unwrap() * 1000;
            let path = lines[line + 1].to_string();

            let s = songs.iter_mut().find(|x| x.path == path);
            if s.is_none() {
                // Song will be added with exists = false
                playlist_songs.push(Song::new(
                    PathBuf::from(&state.base_path).join(&path),
                    &state.base_path,
                    Some(duration),
                ));
                continue;
            }
            let s = s.unwrap();
            if s.duration.is_none() {
                if duration == 0 {
                    s.duration = Some(player::get_duration(
                        &Path::new(&state.base_path).join(&s.path),
                    ));
                } else {
                    s.duration = Some(duration);
                }
            }

            playlist_songs.push(s.clone());
        }
        state
            .playlists
            .push(Playlist::new(playlist_name, playlist_songs));
    }

    // Add All and All Unused playlists
    let mut unused_songs = Vec::new();
    for song in songs.iter_mut() {
        let mut is_song_used = false;
        // Check if song is used in any playlist
        for playlist in state.playlists.iter() {
            let mut hasher = DefaultHasher::new();
            song.hash(&mut hasher);
            let hash = hasher.finish();
            if playlist.songs.iter().any(|s| {
                let mut hasher = DefaultHasher::new();
                s.hash(&mut hasher);
                hasher.finish() == hash
            }) {
                is_song_used = true;
                break;
            }
        }
        if is_song_used {
            continue;
        }

        song.duration = Some(player::get_duration(
            &Path::new(&state.base_path).join(&song.path),
        ));
        unused_songs.push(song.clone());
    }
    state.playlists.push(Playlist::new(
        ALL_UNUSED_PLAYLIST_NAME.to_string(),
        unused_songs,
    ));
    state
        .playlists
        .push(Playlist::new(ALL_PLAYLIST_NAME.to_string(), songs.clone()));

    state.sort_playlists();

    state
}

pub fn draw(ui: &Ui, width: f32, height: f32, state: &mut State, scroll_delta: f32) -> bool {
    //println!("Draw");
    if let Ok(()) = state.song_ended_rx.try_recv() {
        actions::next(state);
    }

    let playlists_width;
    {
        let longest_playlist_name = &state
            .playlists
            .iter()
            .max_by_key(|x| ui.calc_text_size(&x.name)[0].ceil() as usize)
            .unwrap()
            .name;
        playlists_width =
            ui.calc_text_size(format!("{}  XXXX (XXX:XX:XX)", longest_playlist_name))[0].max(350.0);
    }
    let style = ui.clone_style();

    let song_scroll_index = handle_keyboard_shortcuts(ui, state);

    state.has_textbox_focus = false;
    ui.window("main_window")
        .position([0.0, 0.0], Condition::Once)
        .size([width, height], Condition::Always)
        .title_bar(false)
        .resizable(false)
        .movable(false)
        .collapsible(false)
        .draw_background(true)
        .build(|| {
            ui.child_window("playlists")
                .size([playlists_width, height - TEXTBOXES_HEIGHT - CONTROLS_HEIGHT])
                .movable(false)
                .build(|| {
                    ui.get_window_draw_list()
                        .add_rect(
                            [0.0, 0.0],
                            [playlists_width, height - CONTROLS_HEIGHT],
                            PLAYLIST_LIST_BG,
                        )
                        .filled(true)
                        .build();
                    apply_smooth_scrolling(ui, scroll_delta, &mut state.playlists_scroll_info);
                    draw_playlists(ui, state);
                });

            ui.set_cursor_pos([0.0, height - TEXTBOXES_HEIGHT - CONTROLS_HEIGHT]);
            ui.child_window("textboxes")
                .size([playlists_width, TEXTBOXES_HEIGHT])
                .movable(false)
                .build(|| {
                    ui.get_window_draw_list()
                        .add_rect(
                            [0.0, 0.0],
                            [playlists_width, height - CONTROLS_HEIGHT],
                            PLAYLIST_LIST_BG,
                        )
                        .filled(true)
                        .build();
                    draw_textboxes(ui, &style, state);
                });

            let mut scrollbar_width = 0.0;
            ui.set_cursor_pos([playlists_width, SONGS_HEADER_HEIGHT]);
            ui.child_window("songs")
                .size([
                    width - playlists_width,
                    height - CONTROLS_HEIGHT - SONGS_HEADER_HEIGHT,
                ])
                .movable(false)
                .build(|| {
                    apply_smooth_scrolling(ui, scroll_delta, &mut state.songs_scroll_info);
                    if draw_songs(ui, state, song_scroll_index, scroll_delta) {
                        scrollbar_width = style.scrollbar_size
                    }
                });

            ui.set_cursor_pos([playlists_width, 0.0]);
            ui.child_window("songs_header")
                .size([width - playlists_width, SONGS_HEADER_HEIGHT])
                .movable(false)
                .build(|| {
                    ui.get_window_draw_list()
                        .add_rect([0.0, 0.0], [width, SONGS_HEADER_HEIGHT], SONGS_HEADER_BG)
                        .filled(true)
                        .build();
                    draw_songs_header(ui, state, scrollbar_width);
                });

            ui.set_cursor_pos([0.0, height - CONTROLS_HEIGHT]);
            ui.child_window("controls")
                .size([width, CONTROLS_HEIGHT])
                .movable(false)
                .build(|| {
                    ui.get_window_draw_list()
                        .add_rect(
                            [0.0, height - CONTROLS_HEIGHT],
                            [width, height],
                            CONTROLS_BG,
                        )
                        .filled(true)
                        .build();
                    draw_controls(ui, &style, state);
                });

            if !state.dragged_songs.is_empty()
                && (ui.is_mouse_released(MouseButton::Left) || ui.is_key_pressed(Key::Escape))
            {
                ui.reset_mouse_drag_delta(MouseButton::Left);
                state.dragged_songs.clear();
            }

            // Drag
            if ui.is_mouse_dragging(MouseButton::Left) && !state.dragged_songs.is_empty() {
                ui.get_foreground_draw_list()
                    .add_circle(ui.io().mouse_pos, 10.0, DRAG)
                    .filled(true)
                    .build();
                ui.get_foreground_draw_list().add_text(
                    util::add_pos(ui.io().mouse_pos, [5.0, -20.0]),
                    TEXT1,
                    state.dragged_songs.len().to_string(),
                );
            }

            state
                .status_queue
                .retain(|x| (Instant::now() - x.timestamp).as_secs() < 3);
            download::update(state);
            draw_statuses(ui, state);
        });

    state.is_playing
        || state.playlists_scroll_info.is_scrolling
        || state.songs_scroll_info.is_scrolling
        || state.add_to_menu_scroll_info.is_scrolling
}

pub fn handle_keyboard_shortcuts(ui: &Ui, state: &mut State) -> Option<usize> {
    let mut song_scroll_index = None;
    if !state.has_textbox_focus {
        if ui.is_key_pressed_no_repeat(Key::Space) {
            if state.is_playing {
                actions::pause(state);
            } else {
                actions::resume(state);
            }
        }
        if ui.io().key_ctrl && ui.is_key_pressed_no_repeat(Key::RightArrow) {
            actions::next(state);
        }
        if ui.io().key_ctrl && ui.is_key_pressed_no_repeat(Key::LeftArrow) {
            actions::prev(state);
        }
        if ui.io().key_ctrl && ui.is_key_pressed_no_repeat(Key::A) {
            state.selected_song_indices.clear();
            for (i, song) in state.playlists[state.selected_playlist_index]
                .songs
                .iter()
                .enumerate()
            {
                if !state.song_search_text.is_empty() && !song.is_matching(&state.song_search_text)
                {
                    continue;
                }
                state.selected_song_indices.push(i);
            }
        }
        if ui.is_key_pressed_no_repeat(Key::Delete) && !state.selected_song_indices.is_empty() {
            state.selected_song_indices.sort_unstable();
            for i in state.selected_song_indices.iter().rev() {
                // Update playing song index
                if state.playing_song_index == Some(*i) {
                    state.playing_song_index = Some(0.max(state.playing_song_index.unwrap() - 1));
                } else if state.playing_song_index > Some(*i) {
                    state.playing_song_index = Some(state.playing_song_index.unwrap() - 1);
                }

                state.playlists[state.selected_playlist_index]
                    .songs
                    .remove(*i);
            }
            state.selected_song_indices.clear();
        }

        if ui.is_key_pressed(Key::J)
            && !state.selected_song_indices.is_empty()
            && state.song_search_text.is_empty()
            && state.sort_type.is_none()
            && !util::is_default_playlist(&state.playlists[state.selected_playlist_index].name)
        {
            // Move selection down
            state.selected_song_indices.sort_unstable();
            let playlist = &mut state.playlists[state.selected_playlist_index];
            let mut last_index = None;
            for selected_song_index in state.selected_song_indices.iter_mut().rev() {
                if *selected_song_index < playlist.songs.len() - 1
                    && (last_index.is_none() || last_index.unwrap() > *selected_song_index + 1)
                {
                    playlist
                        .songs
                        .swap(*selected_song_index, *selected_song_index + 1);

                    // Update playing song index
                    if state.playing_song_index == Some(*selected_song_index) {
                        state.playing_song_index = Some(*selected_song_index + 1);
                    } else if state.playing_song_index == Some(*selected_song_index + 1) {
                        state.playing_song_index = Some(*selected_song_index);
                    }

                    *selected_song_index += 1;
                }
                last_index = Some(*selected_song_index);
            }
            song_scroll_index = Some(*state.selected_song_indices.last().unwrap());
        }
        if ui.is_key_pressed(Key::K)
            && !state.selected_song_indices.is_empty()
            && state.song_search_text.is_empty()
            && state.sort_type.is_none()
            && !util::is_default_playlist(&state.playlists[state.selected_playlist_index].name)
        {
            // Move selection up
            state.selected_song_indices.sort_unstable();
            let playlist = &mut state.playlists[state.selected_playlist_index];
            let mut last_index = None;
            for selected_song_index in state.selected_song_indices.iter_mut() {
                if *selected_song_index > 0
                    && (last_index.is_none() || last_index.unwrap() < *selected_song_index - 1)
                {
                    playlist
                        .songs
                        .swap(*selected_song_index, *selected_song_index - 1);

                    // Update playing song index
                    if state.playing_song_index == Some(*selected_song_index) {
                        state.playing_song_index = Some(*selected_song_index - 1);
                    } else if state.playing_song_index == Some(*selected_song_index - 1) {
                        state.playing_song_index = Some(*selected_song_index);
                    }

                    *selected_song_index -= 1;
                }
                last_index = Some(*selected_song_index);
            }
            song_scroll_index = Some(*state.selected_song_indices.first().unwrap());
        }
    }
    song_scroll_index
}

fn draw_playlists(ui: &Ui, state: &mut State) {
    let width = ui.window_content_region_max()[0] - ui.window_content_region_min()[0];
    ui.set_cursor_pos([ui.cursor_pos()[0], ui.cursor_pos()[1] + 2.0]);
    let padding_left = 6.0;
    let padding_right = 3.0;
    for i in 0..state.playlists.len() {
        let token = ui.push_id_usize(i);
        // Draw selectable
        if ui
            .selectable_config("")
            .selected(i == state.selected_playlist_index)
            .allow_double_click(true)
            .build()
        {
            let playlist = &state.playlists[i];
            state.selected_playlist_index = i;
            state.selected_song_indices.clear();

            if ui.is_mouse_double_clicked(MouseButton::Left) && !playlist.songs.is_empty() {
                let result = playlist.songs.iter().enumerate().find(|x| x.1.exists);
                if let Some(result) = result {
                    state
                        .action_tx
                        .send(player::PlayerAction::Play(
                            Path::new(&state.base_path).join(&result.1.path),
                        ))
                        .unwrap();
                    state.is_playing = true;
                    state.playing_playlist_index = Some(i);
                    state.playing_song_index = Some(result.0);
                    actions::set_current_metadata(state);
                    state
                        .media_controls
                        .set_playback(MediaPlayback::Playing { progress: None })
                        .unwrap();
                }
            }
        };

        // Drop
        if !state.dragged_songs.is_empty()
            && ui.is_item_visible()
            && util::is_point_in_rect(
                ui.io().mouse_pos,
                util::add_pos(ui.item_rect_min(), [0.0, 1.0]),
                ui.item_rect_max(),
            )
        {
            if ui.is_mouse_released(MouseButton::Left) {
                let song_count = state.dragged_songs.len();
                for dragged_index in (0..song_count).rev() {
                    state.playlists[i]
                        .songs
                        .insert(0, state.dragged_songs.remove(dragged_index));
                }
                actions::increment_indices(state, i, song_count);
            } else {
                ui.get_window_draw_list()
                    .add_rect(
                        util::add_pos(ui.item_rect_min(), [4.0, 1.0]),
                        util::sub_pos(ui.item_rect_max(), [4.0, 0.0]),
                        DRAG,
                    )
                    .build();
            }
        }

        if ui.is_item_clicked_with_button(MouseButton::Right) {
            ui.open_popup("playlist_context_menu");
        }
        ui.popup("playlist_context_menu", || {
            let _style_token = ui.push_style_var(StyleVar::WindowPadding([4.0, 10.0]));
            let playlist = &mut state.playlists[i];
            if ui
                .menu_item_config("Save")
                .enabled(!util::is_default_playlist(&playlist.name))
                .build()
            {
                actions::save_playlist(&state.base_path, playlist);
            }
            ui.menu("Download", || {
                let token = ui.push_id("download_textbox");
                ui.set_next_item_width(500.0);
                if ui
                    .input_text("", &mut state.download_text)
                    .enter_returns_true(true)
                    .hint("URL")
                    .build()
                {
                    state.download_playlist_index = Some(i);
                    download::download(state);
                    ui.close_current_popup();
                }
                state.has_textbox_focus |= ui.is_item_focused();
                token.pop();

                if ui.button("Run") {
                    state.download_playlist_index = Some(i);
                    download::download(state);
                    ui.close_current_popup();
                }
                ui.same_line();
                if ui.button("Cancel") {
                    ui.close_current_popup();
                }
            });
        });

        let playlist = &state.playlists[i];
        let has_changes = if util::is_default_playlist(&playlist.name) {
            false
        } else {
            let mut hasher = DefaultHasher::new();
            for song in playlist.songs.iter() {
                song.hash(&mut hasher);
            }
            playlist.original_hash != hasher.finish()
        };

        // Draw playlist name
        ui.same_line_with_pos(ui.cursor_pos()[0] + padding_left);
        let playlist_name_parts = playlist
            .name
            .split_once('.')
            .unwrap_or(("", &playlist.name));
        let playlist_prefix_text = format!(
            "{}{}",
            if has_changes { "● " } else { "" },
            playlist_name_parts.0
        );
        if state.playing_playlist_index == Some(i) {
            if !playlist_prefix_text.is_empty() {
                ui.text_colored(PLAYING_COLOR, playlist_prefix_text);
                ui.same_line();
            }
            ui.text_colored(PLAYING_COLOR, playlist_name_parts.1);
        } else {
            if !playlist_prefix_text.is_empty() {
                ui.text_colored(DIRECTORY_COLOR, playlist_prefix_text);
                ui.same_line();
            }
            ui.text(playlist_name_parts.1);
        }

        // Draw playlist info
        let duration_sum: u64 = playlist.songs.iter().map(|x| x.duration.unwrap_or(0)).sum();
        let playlist_info = format!(
            "{} ({})",
            playlist.songs.len(),
            util::ms_to_string(duration_sum)
        );

        ui.same_line_with_pos(width - padding_right - ui.calc_text_size(&playlist_info)[0]);
        let color_token = ui.push_style_color(StyleColor::Text, TEXT2);
        ui.text(&playlist_info);
        color_token.pop();

        token.pop();
    }
    ui.set_cursor_pos([ui.cursor_pos()[0], ui.cursor_pos()[1] + 2.0]);
}

fn draw_textboxes(ui: &Ui, style: &Style, state: &mut State) {
    let width = ui.window_content_region_max()[0] - ui.window_content_region_min()[0];
    let style_token = ui.push_style_var(StyleVar::ItemSpacing([1.0, 0.0]));

    let token = ui.push_id("new_playlist_textbox");
    ui.set_next_item_width(width / 2.0);
    if ui
        .input_text("", &mut state.new_playlist_text)
        .enter_returns_true(true)
        .hint(NEW_PLAYLIST_TEXT)
        .build()
    {
        if state
            .playlists
            .iter()
            .any(|x| x.name == state.new_playlist_text)
        {
            return;
        }
        let mut new_playlist = Playlist::new(state.new_playlist_text.clone(), Vec::new());
        new_playlist.original_hash = 0;
        state.playlists.push(new_playlist);
        state.sort_playlists();
        state.new_playlist_text.clear();
    }
    state.has_textbox_focus |= ui.is_item_focused();
    token.pop();

    let token = ui.push_id("song_search_textbox");
    let border_color_token;
    let border_size_token;
    if state.song_search_text.is_empty() {
        border_color_token =
            ui.push_style_color(StyleColor::Border, ui.style_color(StyleColor::Border));
        border_size_token = ui.push_style_var(StyleVar::FrameBorderSize(style.frame_border_size));
    } else {
        border_color_token = ui.push_style_color(StyleColor::Border, PRIMARY1);
        border_size_token = ui.push_style_var(StyleVar::FrameBorderSize(2.0));
    }
    ui.same_line();
    ui.set_next_item_width(width / 2.0);
    if ui
        .input_text("", &mut state.song_search_text)
        .hint(SONG_SEARCH_TEXT)
        .build()
    {
        state.selected_song_indices.clear();
    }
    if !ui.is_item_focused() && ui.io().key_ctrl && ui.is_key_pressed_no_repeat(Key::F) {
        ui.set_keyboard_focus_here_with_offset(FocusedWidget::Previous);
    }
    state.has_textbox_focus |= ui.is_item_focused();
    border_size_token.pop();
    border_color_token.pop();
    token.pop();

    style_token.pop();
}

fn draw_songs_header(ui: &Ui, state: &mut State, scrollbar_offset: f32) {
    let width =
        ui.window_content_region_max()[0] - ui.window_content_region_min()[0] - scrollbar_offset;
    let horizontal_padding = 6.0;
    ui.set_cursor_pos([
        ui.cursor_pos()[0] + horizontal_padding,
        ui.cursor_pos()[1] + 6.0,
    ]);

    // Draw header
    let rect_min = ui.window_pos();
    let rect_max = util::add_pos(ui.window_pos(), [width / 2.0, SONGS_HEADER_HEIGHT]);
    if ui.is_mouse_hovering_rect(rect_min, rect_max) {
        ui.get_window_draw_list()
            .add_rect(rect_min, rect_max, HOVERED_BG)
            .filled(true)
            .build();
        if ui.is_mouse_clicked(MouseButton::Left) {
            state.sort_type = match state.sort_type {
                Some(SortType::Song(SortDirection::Ascending)) => {
                    Some(SortType::Song(SortDirection::Descending))
                }
                Some(SortType::Song(SortDirection::Descending)) => None,
                _ => Some(SortType::Song(SortDirection::Ascending)),
            };
        }
    }
    ui.text("Song");
    match &state.sort_type {
        Some(SortType::Song(sort_direction)) => {
            let icon = sort_direction.get_sort_icon();
            ui.same_line_with_pos(
                rect_max[0] - ui.window_pos()[0] - horizontal_padding - ui.calc_text_size(icon)[0],
            );
            ui.text(icon);
        }
        _ => (),
    };

    let duration_text_x = width
        - 2.0 * horizontal_padding
        - ui.calc_text_size("Duration")[0]
        - SortDirection::get_sort_icon_width(ui);

    let rect_min = util::add_pos(ui.window_pos(), [width / 2.0, 0.0]);
    let rect_max = util::add_pos(ui.window_pos(), [duration_text_x, SONGS_HEADER_HEIGHT]);
    if ui.is_mouse_hovering_rect(rect_min, rect_max) {
        ui.get_window_draw_list()
            .add_rect(rect_min, rect_max, HOVERED_BG)
            .filled(true)
            .build();
        if ui.is_mouse_clicked(MouseButton::Left) {
            state.sort_type = match state.sort_type {
                Some(SortType::Artist(SortDirection::Ascending)) => {
                    Some(SortType::Artist(SortDirection::Descending))
                }
                Some(SortType::Artist(SortDirection::Descending)) => None,
                _ => Some(SortType::Artist(SortDirection::Ascending)),
            };
        }
    }
    ui.same_line_with_pos(width / 2.0 + horizontal_padding);
    ui.text("Artist");
    match &state.sort_type {
        Some(SortType::Artist(sort_direction)) => {
            let icon = sort_direction.get_sort_icon();
            ui.same_line_with_pos(
                rect_max[0] - ui.window_pos()[0] - horizontal_padding - ui.calc_text_size(icon)[0],
            );
            ui.text(icon);
        }
        _ => (),
    };

    let rect_min = util::add_pos(ui.window_pos(), [duration_text_x, 0.0]);
    let rect_max = util::add_pos(ui.window_pos(), [width, SONGS_HEADER_HEIGHT]);
    if ui.is_mouse_hovering_rect(rect_min, rect_max) {
        ui.get_window_draw_list()
            .add_rect(rect_min, rect_max, HOVERED_BG)
            .filled(true)
            .build();
        if ui.is_mouse_clicked(MouseButton::Left) {
            state.sort_type = match state.sort_type {
                Some(SortType::Duration(SortDirection::Ascending)) => {
                    Some(SortType::Duration(SortDirection::Descending))
                }
                Some(SortType::Duration(SortDirection::Descending)) => None,
                _ => Some(SortType::Duration(SortDirection::Ascending)),
            };
        }
    }
    ui.same_line_with_pos(duration_text_x);
    ui.text("Duration");
    match &state.sort_type {
        Some(SortType::Duration(sort_direction)) => {
            let icon = sort_direction.get_sort_icon();
            ui.same_line_with_pos(
                rect_max[0] - ui.window_pos()[0] - horizontal_padding - ui.calc_text_size(icon)[0],
            );
            ui.text(icon);
        }
        _ => (),
    };
}

fn draw_songs(
    ui: &Ui,
    state: &mut State,
    song_scroll_index: Option<usize>,
    scroll_delta: f32,
) -> bool {
    let width = ui.window_content_region_max()[0] - ui.window_content_region_min()[0];
    let horizontal_padding = 6.0;

    ui.set_cursor_pos([ui.cursor_pos()[0], ui.cursor_pos()[1] + 2.0]);
    let draw_list = ui.get_window_draw_list();
    let songs = state.playlists[state.selected_playlist_index].songs.clone();
    let mut counter = 0;

    // TODO Copy a list of all songs here (Vec<&Song>) to handle selection issues etc.
    let song_iter = if state.sort_type.is_none() {
        songs.iter().enumerate().collect()
    } else {
        let mut a: Vec<(usize, &Song)> = songs.iter().enumerate().collect();
        a.sort_by(|a, b| state.sort_type.as_ref().unwrap().compare(a.1, b.1));
        a
    };

    for (sorted_i, (i, song)) in song_iter.iter().enumerate() {
        if !state.song_search_text.is_empty() && !song.is_matching(&state.song_search_text) {
            continue;
        }
        counter += 1;

        let token = ui.push_id_usize(*i);
        // Draw selectable
        draw_list.channels_split(2, |channel| {
            channel.set_current(1);
            if ui
                .selectable_config("")
                .selected(state.selected_song_indices.contains(&i))
                .allow_double_click(true)
                .build()
            {
                if ui.io().key_shift {
                    if !state.selected_song_indices.is_empty() {
                        let first = *state.selected_song_indices.first().unwrap();
                        let sorted_first = song_iter.iter().position(|x| x.0 == first).unwrap();
                        let range = if sorted_first <= sorted_i {
                            (sorted_first + 1)..(sorted_i + 1)
                        } else {
                            sorted_i..sorted_first
                        };

                        state.selected_song_indices.truncate(1);
                        for sorted_idx in range {
                            let idx = song_iter[sorted_idx].0;
                            if !state.song_search_text.is_empty()
                                && !songs[idx].is_matching(&state.song_search_text)
                            {
                                continue;
                            }
                            state.selected_song_indices.push(idx);
                        }
                    }
                } else if ui.io().key_ctrl {
                    if state.selected_song_indices.contains(&i) {
                        let index = state
                            .selected_song_indices
                            .iter()
                            .position(|x| *x == *i)
                            .unwrap();
                        state.selected_song_indices.remove(index);
                    } else {
                        state.selected_song_indices.push(*i);
                    }
                } else {
                    state.selected_song_indices.clear();
                    state.selected_song_indices.push(*i);
                    if ui.is_mouse_double_clicked(MouseButton::Left) && song.exists {
                        state
                            .action_tx
                            .send(player::PlayerAction::Play(
                                Path::new(&state.base_path).join(&song.path),
                            ))
                            .unwrap();
                        state.is_playing = true;
                        state.playing_playlist_index = Some(state.selected_playlist_index);
                        state.playing_song_index = Some(*i);
                        actions::set_current_metadata(state);
                        state
                            .media_controls
                            .set_playback(MediaPlayback::Playing { progress: None })
                            .unwrap();
                    }
                }
            };

            if ui.is_item_clicked_with_button(MouseButton::Right) {
                if !state.selected_song_indices.contains(&i) {
                    state.selected_song_indices.clear();
                }
                if state.selected_song_indices.is_empty() {
                    state.selected_song_indices.push(*i);
                }
                state.original_file_name = state.playlists[state.selected_playlist_index].songs
                    [state.selected_song_indices[0]]
                    .path
                    .clone();
                state.file_name_text = state.original_file_name.clone();
                ui.open_popup("song_context_menu");
            }
            ui.popup("song_context_menu", || {
                let _style_token = ui.push_style_var(StyleVar::WindowPadding([4.0, 10.0]));
                ui.menu("Add to", || {
                    apply_smooth_scrolling(ui, scroll_delta, &mut state.add_to_menu_scroll_info);
                    for playlist_index in 0..state.playlists.len() {
                        let playlist_name = &state.playlists[playlist_index].name;
                        if playlist_name == ALL_PLAYLIST_NAME
                            || playlist_name == ALL_UNUSED_PLAYLIST_NAME
                        {
                            continue;
                        }
                        if ui.menu_item(playlist_name) {
                            state.selected_song_indices.sort_unstable();
                            for i in state.selected_song_indices.iter().rev() {
                                state.playlists[playlist_index]
                                    .songs
                                    .insert(0, songs[*i].clone());
                            }

                            actions::increment_indices(
                                state,
                                playlist_index,
                                state.selected_song_indices.len(),
                            );
                        }
                    }
                });
                if ui.menu_item("Remove") {
                    state.selected_song_indices.sort_unstable();
                    for i in state.selected_song_indices.iter().rev() {
                        // Update playing song index
                        if state.playing_song_index == Some(*i) {
                            state.playing_song_index =
                                Some(0.max(state.playing_song_index.unwrap() - 1));
                        } else if state.playing_song_index > Some(*i) {
                            state.playing_song_index = Some(state.playing_song_index.unwrap() - 1);
                        }

                        state.playlists[state.selected_playlist_index]
                            .songs
                            .remove(*i);
                    }
                    state.selected_song_indices.clear();
                }
                if ui.menu_item("Reload file") {
                    let path = state.playlists[state.selected_playlist_index].songs
                        [state.selected_song_indices[0]]
                        .path
                        .clone();
                    let duration = Some(
                        player::get_duration(&Path::new(&state.base_path).join(&path)) / 1000
                            * 1000,
                    );
                    for playlist in state.playlists.iter_mut() {
                        for song in playlist.songs.iter_mut() {
                            if song.path == *path {
                                song.duration = duration;
                            }
                        }
                    }
                }
                let _disabled_token = ui.begin_disabled(state.selected_song_indices.len() != 1);
                ui.menu("Properties", || {
                    let name_info = &state.file_name_text[..state
                        .file_name_text
                        .rfind('.')
                        .unwrap_or_else(|| state.file_name_text.len())];
                    let name_info: Vec<&str> = name_info.splitn(2, " - ").collect();
                    let artist = name_info[0].trim().to_string();
                    let name = if name_info.len() > 1 {
                        name_info[1].trim().to_string()
                    } else {
                        String::new()
                    };

                    let token = ui.push_id("file_name_textbox");
                    ui.set_next_item_width(500.0);
                    if ui
                        .input_text("", &mut state.file_name_text)
                        .enter_returns_true(true)
                        .build()
                    {
                        actions::change_file_name(state, &artist, &name);
                        ui.close_current_popup();
                    }
                    state.has_textbox_focus |= ui.is_item_focused();
                    token.pop();

                    ui.text("Artist: ");
                    ui.same_line();
                    ui.text(&artist);

                    ui.text("Song: ");
                    ui.same_line();
                    ui.text(&name);

                    if ui.button("Apply") {
                        actions::change_file_name(state, &artist, &name);
                        ui.close_current_popup();
                    }
                    ui.same_line();
                    if ui.button("Cancel") {
                        ui.close_current_popup();
                    }
                });
            });
            channel.set_current(0);
            draw_list
                .add_rect(
                    ui.item_rect_min(),
                    ui.item_rect_max(),
                    if counter % 2 == 1 {
                        SONG_LIST_BG1
                    } else {
                        SONG_LIST_BG2
                    },
                )
                .filled(true)
                .build();
        });

        let color_token = if !song.exists {
            Some(ui.push_style_color(StyleColor::Text, NOT_EXISTING_COLOR))
        } else if state.playing_playlist_index == Some(state.selected_playlist_index)
            && state.playing_song_index == Some(*i)
        {
            Some(ui.push_style_color(StyleColor::Text, PLAYING_COLOR))
        } else {
            None
        };

        // Start dragging (if selection is empty or if selected song is dragged)
        if ui.is_mouse_dragging(MouseButton::Left)
            && ui.is_item_visible()
            && state.dragged_songs.is_empty()
            && util::is_point_in_rect(
                util::sub_pos(ui.io().mouse_pos, ui.mouse_drag_delta()),
                util::add_pos(ui.item_rect_min(), [0.0, 1.0]),
                ui.item_rect_max(),
            )
            && (state.selected_song_indices.is_empty() || state.selected_song_indices.contains(&i))
        {
            if state.selected_song_indices.is_empty() {
                state.selected_song_indices.push(*i);
            }
            for selected_song_index in state.selected_song_indices.iter() {
                state.dragged_songs.push(
                    state.playlists[state.selected_playlist_index].songs[*selected_song_index]
                        .clone(),
                );
            }
        }

        // Draw song name
        ui.same_line_with_pos(ui.cursor_pos()[0] + horizontal_padding);
        draw_truncated_text(ui, &song.name, width / 2.0 - 2.0 * horizontal_padding);

        // Get duration time width
        let song_duration = util::ms_to_string(song.duration.unwrap_or(0));
        let song_duration_width = ui.calc_text_size(&song_duration)[0];

        // Draw song artist
        ui.same_line_with_pos(width / 2.0 + horizontal_padding);
        draw_truncated_text(
            ui,
            &song.artist,
            width / 2.0 - 3.0 * horizontal_padding - song_duration_width,
        );

        // Draw song duration
        ui.same_line_with_pos(width - horizontal_padding - song_duration_width);
        ui.text(&song_duration);
        if let Some(t) = color_token {
            t.pop();
        }
        token.pop();

        if song_scroll_index.is_some()
            && counter == song_scroll_index.unwrap()
            && !ui.is_item_visible()
        {
            ui.set_scroll_here_y();
        }
    }
    ui.set_cursor_pos([ui.cursor_pos()[0], ui.cursor_pos()[1] + 2.0]);
    ui.scroll_max_y() > 0.0
}

fn draw_controls(ui: &Ui, style: &Style, state: &mut State) {
    let width = ui.window_content_region_max()[0] - ui.window_content_region_min()[0];
    let height_middle = CONTROLS_HEIGHT / 2.0;
    ui.columns(5, "control_columns", false);
    ui.set_current_column_width(width / 4.0);
    ui.set_cursor_pos([
        ui.cursor_pos()[0] + width / 8.0 - 75.0,
        ui.cursor_pos()[1] + height_middle - 25.0 - style.item_spacing[1],
    ]);
    let font_token = ui.push_font(ui.fonts().fonts()[1]);
    let style_token = ui.push_style_color(StyleColor::Button, TRANSPARENT);
    let style_token2 = ui.push_style_var(StyleVar::FrameRounding(f32::MAX));
    let text_offset_token = ui.push_style_var(StyleVar::ButtonTextAlign([0.55, 0.9]));
    if ui.button_with_size("⏮", [50.0, 50.0]) {
        actions::prev(state);
    }
    text_offset_token.pop();
    ui.same_line();
    let text_offset_token = ui.push_style_var(StyleVar::ButtonTextAlign([0.55, 0.9]));
    if ui.button_with_size(
        if state.is_playing { "⏸" } else { "▶" }, // ⏮ ▶ ⏸ ⏭
        [50.0, 50.0],
    ) {
        if state.is_playing {
            actions::pause(state);
        } else {
            actions::resume(state);
        }
    }
    text_offset_token.pop();
    ui.same_line();
    let text_offset_token = ui.push_style_var(StyleVar::ButtonTextAlign([0.8, 0.9]));
    if ui.button_with_size("⏭︎", [50.0, 50.0]) {
        actions::next(state);
    }
    text_offset_token.pop();
    font_token.pop();
    style_token.pop();
    style_token2.pop();

    // Current time
    let time_width = 80.0;
    ui.next_column();
    ui.set_current_column_width(time_width);
    let current_time =
        if state.playing_playlist_index.is_some() && state.playing_song_index.is_some() {
            *state.position.lock().unwrap()
        } else {
            0
        };
    let current_time_string = util::ms_to_string(current_time);
    ui.set_cursor_pos([
        ui.cursor_pos()[0] + time_width
            - ui.calc_text_size(&current_time_string)[0]
            - style.item_spacing[0],
        ui.cursor_pos()[1] + height_middle - ui.calc_text_size(&current_time_string)[1] / 2.0 + 1.0,
    ]);
    ui.text(&current_time_string);

    // Song info
    let middle_width = width / 2.0 - 2.0 * time_width;
    ui.next_column();
    ui.set_current_column_width(middle_width);
    let info = if state.playing_playlist_index.is_some() && state.playing_song_index.is_some() {
        let song = &state.playlists[state.playing_playlist_index.unwrap()].songs
            [state.playing_song_index.unwrap()];
        format!("{} - {}", song.artist, song.name)
    } else {
        String::from("-")
    };
    ui.set_cursor_pos([
        ui.cursor_pos()[0] + middle_width / 2.0 - ui.calc_text_size(&info)[0] / 2.0,
        ui.cursor_pos()[1] + height_middle
            - ui.current_font().font_size / 2.0
            - style.item_spacing[1]
            - ui.calc_text_size(&info)[1]
            - 5.0,
    ]);
    ui.text(&info);

    let total_time = if state.playing_playlist_index.is_some() && state.playing_song_index.is_some()
    {
        state.playlists[state.playing_playlist_index.unwrap()].songs
            [state.playing_song_index.unwrap()]
        .duration
        .unwrap()
    } else {
        0
    };
    // Song slider
    let mut progress =
        if state.playing_playlist_index.is_some() && state.playing_song_index.is_some() {
            current_time as f64 / total_time as f64
        } else {
            0.0
        };
    ui.set_cursor_pos([ui.cursor_pos()[0], ui.cursor_pos()[1] + 5.0]);
    let song_slider_pos = ui.cursor_pos();

    let song_slider_width = middle_width - 2.0 * style.item_spacing[0];
    if draw_slider(
        ui,
        "song_slider",
        0.0,
        1.0,
        &mut progress,
        song_slider_width,
        20.0,
    ) {
        state.last_progress = Some(progress);
    }
    if ui.is_item_deactivated_after_edit() && state.last_progress.is_some() {
        let new_position = (state.last_progress.unwrap() * total_time as f64) as u64;
        state
            .action_tx
            .send(player::PlayerAction::Seek(new_position))
            .unwrap();
        *state.position.lock().unwrap() = new_position;
        state.last_progress = None;
    }

    // Draw a rounded rectangle over the slider since there seems to be no other was to make it
    // look filled
    let rect_pos = util::add_pos(
        util::add_pos(ui.window_pos(), util::add_pos(song_slider_pos, [2.0, 2.0])),
        style.window_padding,
    );
    ui.get_window_draw_list()
        .add_rect(
            rect_pos,
            util::add_pos(
                rect_pos,
                [(song_slider_width - 22.0) * progress as f32 + 17.0, 17.0],
            ),
            PRIMARY2,
        )
        .filled(true)
        .thickness(0.0)
        .rounding(f32::MAX)
        .build();

    // Total time
    let total_time_string = util::ms_to_string(total_time);
    ui.next_column();
    ui.set_current_column_width(time_width);
    ui.set_cursor_pos([
        ui.cursor_pos()[0] - style.item_spacing[0],
        ui.cursor_pos()[1] + height_middle - ui.calc_text_size(&total_time_string)[1] / 2.0 + 1.0,
    ]);
    ui.text(&total_time_string);

    // Volume slider
    ui.next_column();
    ui.set_current_column_width(width / 4.0);
    ui.set_cursor_pos([
        ui.cursor_pos()[0] + width / 16.0,
        ui.cursor_pos()[1] + height_middle - ui.current_font().font_size / 2.0,
    ]);
    let volume_slider_pos = ui.cursor_pos();
    if draw_slider(
        ui,
        "volume_slider",
        0.3,
        1.2,
        &mut state.volume,
        width / 8.0,
        20.0,
    ) {
        let value = if state.volume == 0.3 {
            0.0
        } else {
            state.volume.powi(4)
        };
        state
            .action_tx
            .send(player::PlayerAction::SetVolume(value))
            .unwrap();
    }

    // Another rectangle drawn over a slider to make it look filled
    let rect_pos = util::add_pos(
        util::add_pos(
            ui.window_pos(),
            util::add_pos(volume_slider_pos, [2.0, 2.0]),
        ),
        style.window_padding,
    );
    ui.get_window_draw_list()
        .add_rect(
            rect_pos,
            util::add_pos(
                rect_pos,
                [
                    (width / 8.0 - 22.0) * (state.volume - 0.3) * (1.0 / 0.9) + 17.0,
                    17.0,
                ],
            ),
            PRIMARY2,
        )
        .filled(true)
        .thickness(0.0)
        .rounding(f32::MAX)
        .build();
}

fn draw_slider<Data: DataTypeKind>(
    ui: &Ui,
    id: &str,
    min: Data,
    max: Data,
    data: &mut Data,
    width: f32,
    height: f32,
) -> bool {
    ui.set_next_item_width(width);
    let token = ui.push_id(id);
    let style_token = ui.push_style_var(StyleVar::FrameRounding(f32::MAX));
    let style_token2 = ui.push_style_var(StyleVar::GrabRounding(f32::MAX));
    let style_token3 = ui.push_style_var(StyleVar::GrabMinSize(height - 3.0));
    let style_token4 = ui.push_style_var(StyleVar::FramePadding([
        0.0,
        (height - ui.current_font().font_size + 1.0) * 0.5,
    ]));
    let result = ui
        .slider_config("", min, max)
        .display_format("")
        .flags(SliderFlags::NO_INPUT)
        .build(data);
    style_token.pop();
    style_token2.pop();
    style_token3.pop();
    style_token4.pop();
    token.pop();
    result
}

fn draw_statuses(ui: &Ui, state: &mut State) {
    let x_offset = 20.0;
    let padding = 10.0;
    let spacing = 20.0;
    let mut y_offset = 30.0;
    for status in state.status_queue.iter() {
        let rect_size = util::add_pos(
            ui.calc_text_size(&status.info),
            [2.0 * padding, 2.0 * padding],
        );
        ui.get_foreground_draw_list()
            .add_rect(
                [x_offset, y_offset],
                util::add_pos([x_offset, y_offset], rect_size),
                status.r#type.get_color(),
            )
            .filled(true)
            .build();
        ui.get_foreground_draw_list().add_text(
            [x_offset + padding, y_offset + padding],
            TEXT1,
            &status.info,
        );
        y_offset += rect_size[1] + spacing;
    }
}

fn apply_smooth_scrolling(ui: &Ui, scroll_delta: f32, scroll_info: &mut ScrollInfo) {
    let current_time = Instant::now();
    let elapsed_time = current_time.duration_since(scroll_info.scroll_start_time);

    if scroll_info.is_scrolling {
        if elapsed_time < scroll_info.scroll_duration {
            let progress = elapsed_time.as_secs_f32() / scroll_info.scroll_duration.as_secs_f32();
            let mut scroll_y = util::lerp(ui.scroll_y(), scroll_info.scroll_target_y, progress);

            // Clamp values here and to ensure smooth movements at the top/bottom. Clamping the
            // target value when it is set results in an unsmooth transition.
            if scroll_y < 0.0 || scroll_y > ui.scroll_max_y() {
                scroll_y = scroll_y.clamp(0.0, ui.scroll_max_y());
                scroll_info.scroll_target_y =
                    scroll_info.scroll_target_y.clamp(0.0, ui.scroll_max_y());
            }

            ui.set_scroll_y(scroll_y);
        } else {
            scroll_info.is_scrolling = false;
        }
    }

    // Update scroll target when scrolling with the mouse wheel
    if ui.is_window_hovered() {
        if scroll_delta != 0.0 {
            scroll_info.scroll_target_y = if scroll_info.is_scrolling {
                scroll_info.scroll_target_y - scroll_delta * 66.0
            } else {
                ui.scroll_y() - scroll_delta * 66.0
            };
            scroll_info.scroll_start_time = Instant::now();
            scroll_info.is_scrolling = true;
        }
    }
}

fn draw_truncated_text(ui: &Ui, text: &str, width: f32) {
    if ui.calc_text_size(text)[0] <= width {
        ui.text(text);
        return;
    }

    let ellipsis = "...";
    let ellipsis_width = ui.calc_text_size(ellipsis)[0];
    for i in (1..text.len()).rev() {
        let part = text.chars().take(i).collect::<String>();
        if ellipsis_width + ui.calc_text_size(&part)[0] <= width {
            ui.text(format!("{part}{ellipsis}"));
            return;
        }
    }
}
