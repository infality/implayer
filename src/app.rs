use __core::time::Duration;
use souvlaki::{MediaControlEvent, MediaControls, MediaPlayback, PlatformConfig};
use std::{
    cmp::Ordering,
    collections::hash_map::DefaultHasher,
    env, ffi,
    fs::{self, File},
    hash::{Hash, Hasher},
    io::Write,
    path::{Path, PathBuf},
    sync::{
        mpsc::{self, Receiver, Sender},
        Arc, Mutex,
    },
    thread::{self, JoinHandle},
};

use crate::player;
use imgui::*;

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

const ALL_PLAYLIST_NAME: &str = "All";
const ALL_UNUSED_PLAYLIST_NAME: &str = "All Unused";
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
    name: String,
    songs: Vec<Song>,
    original_hash: u64,
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
    path: String,
    name: String,
    artist: String,
    /// Milliseconds
    duration: Option<u64>,
    exists: bool,
}

impl Song {
    pub fn new(path: PathBuf, base_path: &str, duration: Option<u64>) -> Song {
        let file_name = path.file_stem().unwrap().to_string_lossy();
        let name_info: Vec<&str> = file_name.splitn(2, '-').collect();

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

pub struct State {
    base_path: String,
    playlists: Vec<Playlist>,
    selected_playlist_index: usize,
    selected_song_indices: Vec<usize>,
    new_playlist_text: String,
    song_search_text: String,
    has_textbox_focus: bool,

    dragged_songs: Vec<Song>,

    original_file_name: String,
    file_name_text: String,

    playing_playlist_index: Option<usize>,
    playing_song_index: Option<usize>,

    is_playing: bool,
    volume: f32,
    player_thread: JoinHandle<()>,
    action_tx: Sender<player::PlayerAction>,
    song_ended_rx: Receiver<()>,
    last_progress: Option<f64>,
    position: Arc<Mutex<u64>>,
    media_controls: MediaControls,
    media_controls_rx: Receiver<MediaControlEvent>,
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

    if args.len() != 2
        || fs::metadata(&args[1]).is_err()
        || !fs::metadata(&args[1]).unwrap().is_dir()
    {
        println!("Please pass a directory");
        std::process::exit(1);
    }

    let music_extensions = vec!["mp3", "m4a"];

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
        base_path: args[1].clone(),
        playlists: Vec::new(),
        selected_playlist_index: 0,
        selected_song_indices: Vec::new(),
        new_playlist_text: String::new(),
        song_search_text: String::new(),
        has_textbox_focus: false,

        dragged_songs: Vec::new(),

        original_file_name: String::new(),
        file_name_text: String::new(),

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
                    .unwrap()
                    .to_str()
                    .unwrap(),
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

pub fn draw(ui: &Ui, width: f32, height: f32, state: &mut State) -> bool {
    //println!("Draw");
    if let Ok(()) = state.song_ended_rx.try_recv() {
        next(state);
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

    let mut song_scroll_index = None;

    // Handle keyboard shortcuts
    if !state.has_textbox_focus {
        if ui.is_key_pressed_no_repeat(Key::Space) {
            if state.is_playing {
                pause(state);
            } else {
                resume(state);
            }
        }
        if ui.io().key_ctrl && ui.is_key_pressed_no_repeat(Key::RightArrow) {
            next(state);
        }
        if ui.io().key_ctrl && ui.is_key_pressed_no_repeat(Key::LeftArrow) {
            prev(state);
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

        if ui.is_key_index_pressed(glutin::event::VirtualKeyCode::J as u32)
            && !state.selected_song_indices.is_empty()
            && state.song_search_text.is_empty()
            && !is_default_playlist(&state.playlists[state.selected_playlist_index].name)
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
        if ui.is_key_index_pressed(glutin::event::VirtualKeyCode::K as u32)
            && !state.selected_song_indices.is_empty()
            && state.song_search_text.is_empty()
            && !is_default_playlist(&state.playlists[state.selected_playlist_index].name)
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

                    ui.set_cursor_pos([ui.cursor_pos()[0], ui.cursor_pos()[1] + 2.0]);
                    let horizontal_padding = 6.0;
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

                            if ui.is_mouse_double_clicked(MouseButton::Left)
                                && !playlist.songs.is_empty()
                            {
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
                                    set_current_metadata(state);
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
                            && is_point_in_rect(
                                ui.io().mouse_pos,
                                add_pos(ui.item_rect_min(), [0.0, 1.0]),
                                ui.item_rect_max(),
                            )
                        {
                            if ui.is_mouse_released(MouseButton::Left) {
                                for dragged_index in (0..state.dragged_songs.len()).rev() {
                                    state.playlists[i]
                                        .songs
                                        .insert(0, state.dragged_songs.remove(dragged_index));
                                }
                                state.dragged_songs.clear();
                            } else {
                                ui.get_window_draw_list()
                                    .add_rect(
                                        add_pos(ui.item_rect_min(), [4.0, 1.0]),
                                        sub_pos(ui.item_rect_max(), [4.0, 0.0]),
                                        DRAG,
                                    )
                                    .build();
                            }
                        }

                        if ui.is_item_clicked_with_button(MouseButton::Right) {
                            ui.open_popup("playlist_context_menu");
                        }
                        ui.popup("playlist_context_menu", || {
                            let _style_token =
                                ui.push_style_var(StyleVar::WindowPadding([4.0, 10.0]));
                            let playlist = &mut state.playlists[i];
                            if ui
                                .menu_item_config("Save")
                                .enabled(!is_default_playlist(&playlist.name))
                                .build()
                            {
                                save_playlist(&state.base_path, playlist);
                            }
                        });

                        let playlist = &state.playlists[i];
                        let has_changes = if is_default_playlist(&playlist.name) {
                            false
                        } else {
                            let mut hasher = DefaultHasher::new();
                            for song in playlist.songs.iter() {
                                song.hash(&mut hasher);
                            }
                            playlist.original_hash != hasher.finish()
                        };

                        // Draw playlist name
                        ui.same_line_with_pos(ui.cursor_pos()[0] + horizontal_padding);
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
                        let duration_sum: u64 =
                            playlist.songs.iter().map(|x| x.duration.unwrap_or(0)).sum();
                        let playlist_info =
                            format!("{} ({})", playlist.songs.len(), ms_to_string(duration_sum));

                        let scrollbar_width = if ui.scroll_max_y() > 0.0 {
                            style.scrollbar_size
                        } else {
                            0.0
                        };
                        ui.same_line_with_pos(
                            playlists_width
                                - horizontal_padding
                                - ui.calc_text_size(&playlist_info)[0]
                                - scrollbar_width,
                        );
                        let color_token = ui.push_style_color(StyleColor::Text, TEXT2);
                        ui.text(&playlist_info);
                        color_token.pop();

                        token.pop();
                    }
                    ui.set_cursor_pos([ui.cursor_pos()[0], ui.cursor_pos()[1] + 2.0]);
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
                    let style_token = ui.push_style_var(StyleVar::ItemSpacing([1.0, 0.0]));

                    let token = ui.push_id("new_playlist_textbox");
                    ui.set_next_item_width(playlists_width / 2.0);
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
                        let mut new_playlist =
                            Playlist::new(state.new_playlist_text.clone(), Vec::new());
                        new_playlist.original_hash = 0;
                        state.playlists.push(new_playlist);
                        state.sort_playlists();
                        state.new_playlist_text.clear();
                    }
                    state.has_textbox_focus |= ui.is_item_focused();
                    token.pop();

                    let token = ui.push_id("song_search_textbox");
                    ui.same_line();
                    ui.set_next_item_width(playlists_width / 2.0);
                    if ui
                        .input_text("", &mut state.song_search_text)
                        .hint(SONG_SEARCH_TEXT)
                        .build()
                    {
                        state.selected_song_indices.clear();
                    }
                    state.has_textbox_focus |= ui.is_item_focused();
                    token.pop();

                    style_token.pop();
                });

            ui.set_cursor_pos([playlists_width, 0.0]);
            ui.child_window("songs_header")
                .size([width - playlists_width, SONGS_HEADER_HEIGHT])
                .movable(false)
                .build(|| {
                    let window_width =
                        ui.window_content_region_max()[0] - ui.window_content_region_min()[0];
                    ui.get_window_draw_list()
                        .add_rect([0.0, 0.0], [width, SONGS_HEADER_HEIGHT], SONGS_HEADER_BG)
                        .filled(true)
                        .build();

                    let horizontal_padding = 6.0;
                    ui.set_cursor_pos([
                        ui.cursor_pos()[0] + horizontal_padding,
                        ui.cursor_pos()[1] + 6.0,
                    ]);

                    // Draw header
                    let rect_min = ui.window_pos();
                    let rect_max =
                        add_pos(ui.window_pos(), [window_width / 2.0, SONGS_HEADER_HEIGHT]);
                    if ui.is_mouse_hovering_rect(rect_min, rect_max) {
                        ui.get_window_draw_list()
                            .add_rect(rect_min, rect_max, HOVERED_BG)
                            .filled(true)
                            .build();
                    }
                    ui.text("Song");

                    let duration_text_x =
                        window_width - horizontal_padding - ui.calc_text_size("Duration")[0];

                    let rect_min = add_pos(ui.window_pos(), [window_width / 2.0, 0.0]);
                    let rect_max = add_pos(ui.window_pos(), [duration_text_x, SONGS_HEADER_HEIGHT]);
                    if ui.is_mouse_hovering_rect(rect_min, rect_max) {
                        ui.get_window_draw_list()
                            .add_rect(rect_min, rect_max, HOVERED_BG)
                            .filled(true)
                            .build();
                    }
                    ui.same_line_with_pos(window_width / 2.0);
                    ui.text("Artist");

                    let rect_min = add_pos(ui.window_pos(), [duration_text_x, 0.0]);
                    let rect_max = add_pos(ui.window_pos(), [window_width, SONGS_HEADER_HEIGHT]);
                    if ui.is_mouse_hovering_rect(rect_min, rect_max) {
                        ui.get_window_draw_list()
                            .add_rect(rect_min, rect_max, HOVERED_BG)
                            .filled(true)
                            .build();
                    }
                    ui.same_line_with_pos(duration_text_x);
                    ui.text("Duration");
                });

            ui.set_cursor_pos([playlists_width, SONGS_HEADER_HEIGHT]);
            ui.child_window("songs")
                .size([
                    width - playlists_width,
                    height - CONTROLS_HEIGHT - SONGS_HEADER_HEIGHT,
                ])
                .movable(false)
                .build(|| {
                    let window_width =
                        ui.window_content_region_max()[0] - ui.window_content_region_min()[0];
                    let horizontal_padding = 6.0;

                    ui.set_cursor_pos([ui.cursor_pos()[0], ui.cursor_pos()[1] + 2.0]);
                    let draw_list = ui.get_window_draw_list();
                    let songs = state.playlists[state.selected_playlist_index].songs.clone();
                    let mut counter = 0;
                    for (i, song) in songs.iter().enumerate() {
                        if !state.song_search_text.is_empty()
                            && !song.is_matching(&state.song_search_text)
                        {
                            continue;
                        }
                        counter += 1;

                        let token = ui.push_id_usize(i);
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
                                        let range = if first <= i {
                                            (first + 1)..(i + 1)
                                        } else {
                                            i..first
                                        };

                                        state.selected_song_indices.truncate(1);
                                        for j in range {
                                            state.selected_song_indices.push(j);
                                        }
                                    }
                                } else if ui.io().key_ctrl {
                                    if state.selected_song_indices.contains(&i) {
                                        let index = state
                                            .selected_song_indices
                                            .iter()
                                            .position(|x| *x == i)
                                            .unwrap();
                                        state.selected_song_indices.remove(index);
                                    } else {
                                        state.selected_song_indices.push(i);
                                    }
                                } else {
                                    state.selected_song_indices.clear();
                                    state.selected_song_indices.push(i);
                                    if ui.is_mouse_double_clicked(MouseButton::Left) && song.exists
                                    {
                                        state
                                            .action_tx
                                            .send(player::PlayerAction::Play(
                                                Path::new(&state.base_path).join(&song.path),
                                            ))
                                            .unwrap();
                                        state.is_playing = true;
                                        state.playing_playlist_index =
                                            Some(state.selected_playlist_index);
                                        state.playing_song_index = Some(i);
                                        set_current_metadata(state);
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
                                    state.selected_song_indices.push(i);
                                }
                                state.original_file_name = state.playlists
                                    [state.selected_playlist_index]
                                    .songs[state.selected_song_indices[0]]
                                    .path
                                    .clone();
                                state.file_name_text = state.original_file_name.clone();
                                ui.open_popup("song_context_menu");
                            }
                            ui.popup("song_context_menu", || {
                                let _style_token =
                                    ui.push_style_var(StyleVar::WindowPadding([4.0, 10.0]));
                                ui.menu("Add to", || {
                                    for (playlist_index, playlist) in
                                        state.playlists.iter_mut().enumerate()
                                    {
                                        if playlist.name == ALL_PLAYLIST_NAME
                                            || playlist.name == ALL_UNUSED_PLAYLIST_NAME
                                        {
                                            continue;
                                        }
                                        if ui.menu_item(&playlist.name) {
                                            state.selected_song_indices.sort_unstable();
                                            for i in state.selected_song_indices.iter().rev() {
                                                playlist.songs.insert(0, songs[*i].clone());

                                                // Update playing song index
                                                if state.playing_playlist_index
                                                    == Some(playlist_index)
                                                    && state.playing_song_index.is_some()
                                                {
                                                    state.playing_song_index =
                                                        Some(state.playing_song_index.unwrap() + 1);
                                                }
                                            }
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
                                            state.playing_song_index =
                                                Some(state.playing_song_index.unwrap() - 1);
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
                                        player::get_duration(
                                            &Path::new(&state.base_path).join(&path),
                                        ) / 1000
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
                                let _disabled_token =
                                    ui.begin_disabled(state.selected_song_indices.len() != 1);
                                ui.menu("Properties", || {
                                    let name_info = &state.file_name_text[..state
                                        .file_name_text
                                        .rfind('.')
                                        .unwrap_or_else(|| state.file_name_text.len())];
                                    let name_info: Vec<&str> = name_info.splitn(2, '-').collect();
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
                                        change_file_name(state, &artist, &name);
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
                                        change_file_name(state, &artist, &name);
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
                                    if i % 2 == 0 {
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
                        } else if state.playing_playlist_index
                            == Some(state.selected_playlist_index)
                            && state.playing_song_index == Some(i)
                        {
                            Some(ui.push_style_color(StyleColor::Text, PLAYING_COLOR))
                        } else {
                            None
                        };

                        // Start dragging (if selection is empty or if selected song is dragged)
                        if ui.is_mouse_dragging(MouseButton::Left)
                            && ui.is_item_visible()
                            && state.dragged_songs.is_empty()
                            && is_point_in_rect(
                                sub_pos(ui.io().mouse_pos, ui.mouse_drag_delta()),
                                add_pos(ui.item_rect_min(), [0.0, 1.0]),
                                ui.item_rect_max(),
                            )
                            && (state.selected_song_indices.is_empty()
                                || state.selected_song_indices.contains(&i))
                        {
                            if state.selected_song_indices.is_empty() {
                                state.selected_song_indices.push(i);
                            }
                            for selected_song_index in state.selected_song_indices.iter() {
                                state.dragged_songs.push(
                                    state.playlists[state.selected_playlist_index].songs
                                        [*selected_song_index]
                                        .clone(),
                                );
                            }
                        }

                        // Draw song name
                        ui.same_line_with_pos(ui.cursor_pos()[0] + horizontal_padding);
                        ui.text(&song.name);

                        // Draw song artist
                        ui.same_line_with_pos(window_width / 2.0);
                        ui.text(&song.artist);

                        // Draw song duration
                        let song_duration = ms_to_string(song.duration.unwrap_or(0));

                        ui.same_line_with_pos(
                            window_width
                                - horizontal_padding
                                - ui.calc_text_size(&song_duration)[0],
                        );
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

                    let height_middle = CONTROLS_HEIGHT / 2.0;
                    ui.columns(5, "control_columns", false);
                    ui.set_current_column_width(width / 4.0);
                    ui.set_cursor_pos([
                        ui.cursor_pos()[0] + width / 8.0 - 75.0,
                        ui.cursor_pos()[1] + height_middle - 25.0 - style.item_spacing[1],
                    ]);
                    let font_token = ui.push_font(ui.fonts().fonts()[1]);
                    if ui.button_with_size("⏮", [50.0, 50.0]) {
                        prev(state);
                    }
                    ui.same_line();
                    if ui.button_with_size(
                        if state.is_playing { "⏸" } else { "▶" }, // ⏮ ▶ ⏸ ⏭
                        [50.0, 50.0],
                    ) {
                        if state.is_playing {
                            pause(state);
                        } else {
                            resume(state);
                        }
                    }
                    ui.same_line();
                    if ui.button_with_size("⏭︎", [50.0, 50.0]) {
                        next(state);
                    }
                    font_token.pop();

                    // Current time
                    let time_width = 80.0;
                    ui.next_column();
                    ui.set_current_column_width(time_width);
                    let current_time = if state.playing_playlist_index.is_some()
                        && state.playing_song_index.is_some()
                    {
                        *state.position.lock().unwrap()
                    } else {
                        0
                    };
                    let current_time_string = ms_to_string(current_time);
                    ui.set_cursor_pos([
                        ui.cursor_pos()[0] + time_width
                            - ui.calc_text_size(&current_time_string)[0]
                            - style.item_spacing[0],
                        ui.cursor_pos()[1] + height_middle
                            - ui.calc_text_size(&current_time_string)[1] / 2.0
                            + 3.0,
                    ]);
                    ui.text(&current_time_string);

                    // Song info
                    let middle_width = width / 2.0 - 2.0 * time_width;
                    ui.next_column();
                    ui.set_current_column_width(middle_width);
                    let info = if state.playing_playlist_index.is_some()
                        && state.playing_song_index.is_some()
                    {
                        let song = &state.playlists[state.playing_playlist_index.unwrap()].songs
                            [state.playing_song_index.unwrap()];
                        format!("{} - {}", song.name, song.artist)
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

                    let total_time = if state.playing_playlist_index.is_some()
                        && state.playing_song_index.is_some()
                    {
                        state.playlists[state.playing_playlist_index.unwrap()].songs
                            [state.playing_song_index.unwrap()]
                        .duration
                        .unwrap()
                    } else {
                        0
                    };
                    // Song slider
                    let mut progress = if state.playing_playlist_index.is_some()
                        && state.playing_song_index.is_some()
                    {
                        current_time as f64 / total_time as f64
                    } else {
                        0.0
                    };
                    ui.set_cursor_pos([ui.cursor_pos()[0], ui.cursor_pos()[1] + 5.0]);
                    ui.set_next_item_width(middle_width - 2.0 * style.item_spacing[0]);
                    let token = ui.push_id("song_slider");
                    if ui
                        .slider_config("", 0.0, 1.0)
                        .display_format("")
                        .flags(SliderFlags::NO_INPUT)
                        .build(&mut progress)
                    {
                        state.last_progress = Some(progress);
                    }
                    if ui.is_item_deactivated_after_edit() && state.last_progress.is_some() {
                        let new_position =
                            (state.last_progress.unwrap() * total_time as f64) as u64;
                        state
                            .action_tx
                            .send(player::PlayerAction::Seek(new_position))
                            .unwrap();
                        *state.position.lock().unwrap() = new_position;
                        state.last_progress = None;
                    }
                    token.pop();

                    // Total time
                    let total_time_string = ms_to_string(total_time);
                    ui.next_column();
                    ui.set_current_column_width(time_width);
                    ui.set_cursor_pos([
                        ui.cursor_pos()[0] - style.item_spacing[0],
                        ui.cursor_pos()[1] + height_middle
                            - ui.calc_text_size(&total_time_string)[1] / 2.0
                            + 3.0,
                    ]);
                    ui.text(&total_time_string);

                    // Volume slider
                    ui.next_column();
                    ui.set_current_column_width(width / 4.0);
                    ui.set_cursor_pos([
                        ui.cursor_pos()[0] + width / 16.0,
                        ui.cursor_pos()[1] + height_middle - ui.current_font().font_size / 2.0,
                    ]);
                    ui.set_next_item_width(width / 8.0);
                    let token = ui.push_id("volume_slider");
                    if ui
                        .slider_config("", 0.4, 1.2)
                        .display_format("")
                        .flags(SliderFlags::NO_INPUT)
                        .build(&mut state.volume)
                    {
                        let value = if state.volume == 0.4 {
                            0.0
                        } else {
                            state.volume.powi(4)
                        };
                        state
                            .action_tx
                            .send(player::PlayerAction::SetVolume(value))
                            .unwrap();
                    }
                    token.pop();
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
                    add_pos(ui.io().mouse_pos, [5.0, -20.0]),
                    TEXT1,
                    state.dragged_songs.len().to_string(),
                );
            }
        });
    state.is_playing
}

pub fn handle_media_keys(state: &mut State) {
    match state.media_controls_rx.try_recv() {
        Ok(MediaControlEvent::Toggle) => {
            if state.is_playing {
                pause(state);
            } else {
                resume(state);
            }
        }
        Ok(MediaControlEvent::Play) => resume(state),
        Ok(MediaControlEvent::Pause) => pause(state),
        Ok(MediaControlEvent::Next) => next(state),
        Ok(MediaControlEvent::Previous) => prev(state),
        Ok(MediaControlEvent::Stop) => stop(state),
        Ok(MediaControlEvent::Seek(_)) => (),
        Ok(MediaControlEvent::SeekBy(_, _)) => (),
        Ok(MediaControlEvent::SetPosition(_)) => (),
        Ok(MediaControlEvent::OpenUri(_)) => (),
        Ok(MediaControlEvent::Raise) => (),
        Ok(MediaControlEvent::Quit) => (),
        Err(_) => (),
    }
}

pub fn set_current_metadata(state: &mut State) {
    let current_song = &state.playlists[state.playing_playlist_index.unwrap()].songs
        [state.playing_song_index.unwrap()];
    state
        .media_controls
        .set_metadata(souvlaki::MediaMetadata {
            title: Some(&current_song.name),
            album: Some(""),
            artist: Some(&current_song.artist),
            cover_url: None,
            duration: current_song.duration.map(Duration::from_millis),
        })
        .unwrap();
}

fn change_file_name(state: &mut State, artist: &str, name: &str) {
    let exists = Path::new(&state.base_path)
        .join(&state.original_file_name)
        .exists();
    if exists {
        fs::rename(
            &Path::new(&state.base_path).join(&state.original_file_name),
            &Path::new(&state.base_path).join(&state.file_name_text),
        )
        .unwrap();
    }

    let exists = Path::new(&state.base_path)
        .join(&state.file_name_text)
        .exists();
    for playlist in state.playlists.iter_mut() {
        for song in playlist.songs.iter_mut() {
            if song.path == state.original_file_name {
                song.path = state.file_name_text.clone();
                song.artist = artist.to_string();
                song.name = name.to_string();
                song.exists = exists;
            }
        }
    }
}

fn play(state: &mut State, playlist_index: usize, song_index: usize) {
    let song = &state.playlists[playlist_index].songs[song_index];
    if !song.exists {
        return;
    }
    state
        .action_tx
        .send(player::PlayerAction::Play(
            Path::new(&state.base_path).join(&song.path),
        ))
        .unwrap();
    state.is_playing = true;
    state.playing_playlist_index = Some(playlist_index);
    state.playing_song_index = Some(song_index);
    set_current_metadata(state);
    state
        .media_controls
        .set_playback(MediaPlayback::Playing { progress: None })
        .unwrap();
}

fn pause(state: &mut State) {
    state.action_tx.send(player::PlayerAction::Pause).unwrap();
    state.is_playing = false;
    state
        .media_controls
        .set_playback(MediaPlayback::Paused { progress: None })
        .unwrap();
}

fn stop(state: &mut State) {
    state.action_tx.send(player::PlayerAction::Stop).unwrap();
    state.is_playing = false;
    state.playing_playlist_index = None;
    state.playing_song_index = None;
    state
        .media_controls
        .set_playback(MediaPlayback::Stopped)
        .unwrap();
}

fn resume(state: &mut State) {
    if state.playing_song_index.is_none() {
        return;
    }
    state.action_tx.send(player::PlayerAction::Resume).unwrap();
    state.is_playing = true;
    state
        .media_controls
        .set_playback(MediaPlayback::Playing { progress: None })
        .unwrap();
}

fn prev(state: &mut State) {
    if state.playing_playlist_index.is_none() || state.playing_song_index.is_none() {
        return;
    }
    let mut prev_song_index = None;
    let mut prev_song = None;
    let playlist = &state.playlists[state.playing_playlist_index.unwrap()];
    for (i, song) in playlist
        .songs
        .iter()
        .rev()
        .enumerate()
        .skip(playlist.songs.len() - state.playing_song_index.unwrap())
    {
        if song.exists {
            prev_song_index = Some(playlist.songs.len() - i - 1);
            prev_song = Some(song);
            break;
        }
    }

    if prev_song.is_none() {
        stop(state);
        return;
    }

    state
        .action_tx
        .send(player::PlayerAction::Play(
            Path::new(&state.base_path).join(&prev_song.unwrap().path),
        ))
        .unwrap();
    state.is_playing = true;
    state.playing_song_index = prev_song_index;
    set_current_metadata(state);
}

fn next(state: &mut State) {
    if state.playing_playlist_index.is_none() || state.playing_song_index.is_none() {
        return;
    }
    let mut next_song_index = None;
    let mut next_song = None;
    let playlist = &state.playlists[state.playing_playlist_index.unwrap()];
    for (i, song) in playlist
        .songs
        .iter()
        .enumerate()
        .skip(state.playing_song_index.unwrap() + 1)
    {
        if song.exists {
            next_song_index = Some(i);
            next_song = Some(song);
            break;
        }
    }

    if next_song.is_none() {
        stop(state);
        return;
    }

    state
        .action_tx
        .send(player::PlayerAction::Play(
            Path::new(&state.base_path).join(&next_song.unwrap().path),
        ))
        .unwrap();
    state.is_playing = true;
    state.playing_song_index = next_song_index;
    set_current_metadata(state);
}

fn save_playlist(base_path: &str, playlist: &mut Playlist) {
    let mut file =
        File::create(Path::new(base_path).join(format!("{}.m3u", &playlist.name))).unwrap();
    write!(file, "#EXTM3U").unwrap();
    for song in playlist.songs.iter() {
        write!(
            file,
            "\n#EXTINF:{},{} - {}\n{}",
            song.duration.unwrap_or(0) / 1000,
            song.artist,
            song.name,
            song.path,
        )
        .unwrap();
    }
    file.flush().unwrap();

    let mut hasher = DefaultHasher::new();
    for song in playlist.songs.iter() {
        song.hash(&mut hasher);
    }
    playlist.original_hash = hasher.finish();
}

fn is_default_playlist(playlist_name: &str) -> bool {
    playlist_name == ALL_PLAYLIST_NAME || playlist_name == ALL_UNUSED_PLAYLIST_NAME
}

fn ms_to_string(milli_seconds: u64) -> String {
    let mut result = String::new();

    let hour = 1000 * 60 * 60;
    let minute = 1000 * 60;
    let second = 1000;

    if milli_seconds >= hour {
        result += &(milli_seconds / hour).to_string();
        result += ":"
    }

    if milli_seconds >= minute {
        if milli_seconds >= hour {
            result += &format!("{:02}", ((milli_seconds % hour) / minute));
        } else {
            result += &((milli_seconds % hour) / minute).to_string();
        }
        result += ":"
    } else {
        result += "0:"
    }

    result += &format!("{:02}", ((milli_seconds % minute) / second));

    result
}

fn add_pos(first: [f32; 2], second: [f32; 2]) -> [f32; 2] {
    [first[0] + second[0], first[1] + second[1]]
}

fn sub_pos(first: [f32; 2], second: [f32; 2]) -> [f32; 2] {
    [first[0] - second[0], first[1] - second[1]]
}

fn is_point_in_rect(point: [f32; 2], rect_min: [f32; 2], rect_max: [f32; 2]) -> bool {
    if point[0] < rect_min[0] || point[1] < rect_min[1] {
        return false;
    }
    if point[0] > rect_max[0] || point[1] > rect_max[1] {
        return false;
    }
    true
}
