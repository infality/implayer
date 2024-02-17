# ImPlayer

A music player to play and organize m3u8 playlists on Linux/Windows written in Rust using ImGui bindings.

## Features

* Play playlists
* Playlist management (search, sort, add and remove songs or adjust their order)
* Rename song files
* Supports flac, mp3, m4a, ogg and wav files
* Download songs (requires yt-dlp and aacgain)
    * Automatically runs aacgain afterwards to adjust the volume level of the music file

## Usage

Build: `cargo build --release`

Run: Pass the music directory as argument

Hotkeys:
* `Space` Resume/pause playback
* `Ctrl+Left`/`Ctrl+Right` Play previous/next song
* `J`/`K` Move selected songs up/down
* `Delete` Remove song from playlist
* `Ctrl+Click`/`Shift+Click` Extended selection
* `Right-click` Context menu for more options
* `Ctrl+F` Focus search field

Songs can be moved to other playlists via drag and drop and many of the above actions can also be performed through the context menu (right click).

## Screenshot

![screenshot](implayer.png)

## Why?

I want to synchronize my music and playlists across multiple devices (Android, Linux and Windows) using a file synchronization tool.
However, I could not find a suitable modern cross-platform music player that manages playlists only through m3u8 playlist files.

## Roadmap

Things that will be tackled eventually:
* Improve error handling when downloading
* Queue functionality
* Random playback
* Better keyboard movement (e.g. select playlists/songs using arrow keys)
* Drag&Drop within a playlist
