use std::{
    io::{BufRead, BufReader, Read},
    process::{Child, Command, Stdio},
    sync::mpsc::{self, Receiver, Sender},
    thread::{self},
    time::Instant,
};

use crate::{
    actions,
    app::{DownloadState, State, Status, StatusType},
    util,
};

fn start_download(base_path: &str, url: &str) -> Child {
    Command::new("yt-dlp")
        .arg("-o")
        .arg(format!("{}/%(title)s.%(ext)s", base_path))
        .arg("--print")
        .arg("%(filename)s")
        .arg("-q")
        .arg("--no-simulate")
        .arg("--no-playlist")
        .arg("-f")
        .arg("ba[ext=m4a] / ba[ext=mp3]")
        .arg("--progress")
        .arg("--newline")
        .arg("--progress-template")
        .arg("#status#%(progress._percent_str)s")
        .arg(url)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap()
}

fn start_postprocessing(path: &str) -> Child {
    Command::new("aacgain")
        .arg("-r")
        .arg(path)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap()
}

fn start_listener<R: Read + std::marker::Send + 'static>(
    stdout: R,
    sender: Sender<String>,
    receiver: Receiver<()>,
) {
    thread::spawn(move || {
        let mut f = BufReader::new(stdout);
        loop {
            if let Ok(()) = receiver.try_recv() {
                return;
            }
            let mut buf = String::new();
            f.read_line(&mut buf).unwrap();
            if buf.trim().len() > 0 {
                if let Err(_) = sender.send(buf) {
                    return;
                }
            }
        }
    });
}

pub fn download(state: &mut State) {
    if !matches!(state.download_state, DownloadState::None) {
        return;
    }
    let now = Instant::now();
    let mut child = start_download(&state.base_path, &state.download_text);
    state.status_queue.push_back(Status {
        info: "Starting download...".to_string(),
        timestamp: now,
        r#type: StatusType::Info,
    });
    state.last_download_status = None;
    let (stdout_tx, stdout_rx) = mpsc::channel();
    let (stdout_kill_tx, stdout_kill_rx) = mpsc::channel();
    start_listener(child.stdout.take().unwrap(), stdout_tx, stdout_kill_rx);
    let (stderr_tx, stderr_rx) = mpsc::channel();
    let (stderr_kill_tx, stderr_kill_rx) = mpsc::channel();
    start_listener(child.stderr.take().unwrap(), stderr_tx, stderr_kill_rx);
    state.download_state =
        DownloadState::Downloading(child, stdout_rx, stdout_kill_tx, stderr_rx, stderr_kill_tx);
}

pub fn update(state: &mut State) {
    let now = Instant::now();
    match state.download_state {
        DownloadState::None => (),
        DownloadState::Downloading(
            ref mut child,
            ref stdout_rx,
            ref stdout_kill_tx,
            ref stderr_rx,
            ref stderr_kill_tx,
        ) => {
            // Report progress once every few seconds
            for line in util::receive_all(stdout_rx).iter().rev() {
                if line.trim().len() == 0 {
                    continue;
                }
                if line.starts_with("#status#") {
                    if state.last_download_status.is_none()
                        || (now - state.last_download_status.unwrap()).as_millis() >= 100
                    {
                        state
                            .status_queue
                            .retain(|x| !matches!(x.r#type, StatusType::Progress));
                        state.status_queue.push_back(Status {
                            info: format!("Download progress: {}", line[8..].trim()),
                            timestamp: now,
                            r#type: StatusType::Progress,
                        });
                        state.last_download_status = Some(now);
                    }
                } else {
                    state.download_path = Some(line.trim().to_string());
                }
            }

            // Check if child exited
            if let Ok(Some(status)) = child.try_wait() {
                stdout_kill_tx.send(()).unwrap();
                stderr_kill_tx.send(()).unwrap();
                if status.success() {
                    state.status_queue.push_back(Status {
                        info: "Download finished, starting postprocessing...".to_string(),
                        timestamp: now,
                        r#type: StatusType::Info,
                    });

                    // Start postprocessing
                    let mut new_child =
                        start_postprocessing(&state.download_path.as_ref().unwrap());
                    state.last_download_status = None;
                    let (stdout_tx, stdout_rx) = mpsc::channel();
                    let (stdout_kill_tx, stdout_kill_rx) = mpsc::channel();
                    start_listener(new_child.stdout.take().unwrap(), stdout_tx, stdout_kill_rx);
                    let (stderr_tx, stderr_rx) = mpsc::channel();
                    let (stderr_kill_tx, stderr_kill_rx) = mpsc::channel();
                    start_listener(new_child.stderr.take().unwrap(), stderr_tx, stderr_kill_rx);
                    state.download_state = DownloadState::Postprocessing(
                        new_child,
                        stdout_rx,
                        stdout_kill_tx,
                        stderr_rx,
                        stderr_kill_tx,
                    );
                } else {
                    let error = util::receive_all(stderr_rx).join("\n");
                    state.status_queue.push_back(Status {
                        info: format!("Error while downloading:\n{error}"),
                        timestamp: now,
                        r#type: StatusType::Error,
                    });
                    state.download_state = DownloadState::None;
                }
            }
        }
        DownloadState::Postprocessing(
            ref mut child,
            ref stdout_rx,
            ref stdout_kill_tx,
            ref stderr_rx,
            ref stderr_kill_tx,
        ) => {
            // Check if child exited
            if let Ok(Some(status)) = child.try_wait() {
                stdout_kill_tx.send(()).unwrap();
                stderr_kill_tx.send(()).unwrap();
                if status.success() {
                    state.status_queue.push_back(Status {
                        info: "Postprocessing finished".to_string(),
                        timestamp: now,
                        r#type: StatusType::Info,
                    });

                    actions::add_song(
                        state,
                        &state.download_path.clone().unwrap(),
                        state.download_playlist_index.unwrap(),
                    );
                    state.download_text = String::new();
                } else {
                    let error = util::receive_all(stdout_rx).join("\n");
                    state.status_queue.push_back(Status {
                        info: format!("Error during postprocessing:\n{error}"),
                        timestamp: now,
                        r#type: StatusType::Error,
                    });
                }
                state.download_state = DownloadState::None;
            } else {
                // Report progress once every few seconds
                for line in util::receive_all(stdout_rx).iter().rev() {
                    if line.trim().len() == 0 {
                        continue;
                    }
                    let percent_char = line.chars().nth(2);
                    if percent_char.is_some() && percent_char.unwrap() == '%' {
                        if state.last_download_status.is_none()
                            || (now - state.last_download_status.unwrap()).as_millis() >= 100
                        {
                            state
                                .status_queue
                                .retain(|x| !matches!(x.r#type, StatusType::Progress));
                            state.status_queue.push_back(Status {
                                info: format!("Postprocessing progress: {}", line[..3].trim()),
                                timestamp: now,
                                r#type: StatusType::Progress,
                            });
                            state.last_download_status = Some(now);
                        }
                    }
                }
            }
        }
    };
}
