use std::sync::mpsc::Receiver;

pub fn ms_to_string(milli_seconds: u64) -> String {
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

pub fn add_pos(first: [f32; 2], second: [f32; 2]) -> [f32; 2] {
    [first[0] + second[0], first[1] + second[1]]
}

pub fn sub_pos(first: [f32; 2], second: [f32; 2]) -> [f32; 2] {
    [first[0] - second[0], first[1] - second[1]]
}

pub fn is_point_in_rect(point: [f32; 2], rect_min: [f32; 2], rect_max: [f32; 2]) -> bool {
    if point[0] < rect_min[0] || point[1] < rect_min[1] {
        return false;
    }
    if point[0] > rect_max[0] || point[1] > rect_max[1] {
        return false;
    }
    true
}

pub fn is_default_playlist(playlist_name: &str) -> bool {
    playlist_name == crate::app::ALL_PLAYLIST_NAME
        || playlist_name == crate::app::ALL_UNUSED_PLAYLIST_NAME
}

pub fn lerp(start: f32, end: f32, t: f32) -> f32 {
    start + t * (end - start)
}

pub fn receive_all<T>(receiver: &Receiver<T>) -> Vec<T> {
    let mut result = Vec::new();
    while let Ok(value) = receiver.try_recv() {
        result.push(value);
    }
    result
}
