use std::{
    collections::hash_map::DefaultHasher,
    fs::{self, File},
    hash::{Hash, Hasher},
    io::Write,
    path::{Path, PathBuf},
    time::Duration,
};

use souvlaki::{MediaControlEvent, MediaPlayback};

use crate::{
    app::{self, Playlist, Song, State},
    player,
};

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

pub fn change_file_name(state: &mut State, artist: &str, name: &str) {
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

pub fn increment_indices(state: &mut State, playlist_index: usize, amount: usize) {
    // Update selected song indices
    if state.selected_playlist_index == playlist_index && !state.selected_song_indices.is_empty() {
        for i in state.selected_song_indices.iter_mut() {
            *i += amount;
        }
    }

    // Update playing song index
    if state.playing_playlist_index == Some(playlist_index) && state.playing_song_index.is_some() {
        state.playing_song_index = Some(state.playing_song_index.unwrap() + amount);
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

pub fn pause(state: &mut State) {
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

pub fn resume(state: &mut State) {
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

pub fn prev(state: &mut State) {
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

pub fn next(state: &mut State) {
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

pub fn save_playlist(base_path: &str, playlist: &mut Playlist) {
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

pub fn add_song(state: &mut State, path: &str, playlist_index: usize) {
    let path = PathBuf::from(path);
    let duration = Some(player::get_duration(&path));
    let song = Song::new(path, &state.base_path, duration);

    state.playlists[playlist_index]
        .songs
        .insert(0, song.clone());
    state
        .playlists
        .iter_mut()
        .find(|x| x.name == app::ALL_PLAYLIST_NAME)
        .unwrap()
        .songs
        .push(song);

    increment_indices(state, playlist_index, 1);
}
