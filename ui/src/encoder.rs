//! Turning pictures into terminal graphics in a background thread: about
//! 10 ms with the Kitty protocol, but 120 to 150 ms with Sixel.

use std::sync::Arc;
use std::sync::mpsc::{self, Receiver, Sender};
use std::thread;
use std::time::{Duration, Instant};

use corrode_core::picture::Picture;
use image::imageops::FilterType;
use ratatui::layout::Size;
use ratatui_image::Resize;
use ratatui_image::picker::Picker;
use ratatui_image::protocol::Protocol;

/// What to draw: a picture, or a part of it, in an area of the screen.
#[derive(Clone)]
pub struct Request {
    pub picture: Arc<Picture>,
    /// `(x, y, width, height)` of the part to show, each of its pixels
    /// taking `scale` screen pixels; the whole picture, scaled to the
    /// area, when `None`.
    pub crop: Option<(u32, u32, u32, u32)>,
    pub scale: u32,
    pub area: Size,
}

impl PartialEq for Request {
    fn eq(&self, other: &Request) -> bool {
        Arc::ptr_eq(&self.picture, &other.picture)
            && self.crop == other.crop
            && self.scale == other.scale
            && self.area == other.area
    }
}

pub struct Encoded {
    pub request: Request,
    pub protocol: Result<Protocol, String>,
    pub elapsed: Duration,
}

pub struct Encoder {
    requests: Sender<Request>,
}

impl Encoder {
    pub fn new(picker: Picker, results: Sender<Encoded>) -> Encoder {
        let (requests, received) = mpsc::channel();
        thread::spawn(move || encode_loop(&picker, &received, &results));
        Encoder { requests }
    }

    pub fn request(&self, request: Request) {
        // The thread only stops when the encoder is dropped.
        let _ = self.requests.send(request);
    }
}

/// How a picture is fitted to its area. At 100% it is already the size of
/// the area and is never scaled; otherwise it is scaled with a filter that
/// averages: the default of the terminal graphics library is nearest
/// neighbour, which keeps one pixel in two or three and turns a face into
/// speckle, and the dither and the noise of a dark picture into false
/// bands.
pub fn resize_for(at_100_percent: bool) -> Resize {
    if at_100_percent {
        Resize::Crop(None)
    } else {
        Resize::Scale(Some(FilterType::Lanczos3))
    }
}

fn encode_loop(picker: &Picker, requests: &Receiver<Request>, results: &Sender<Encoded>) {
    while let Ok(mut request) = requests.recv() {
        // Only the latest request matters: the others are already stale.
        while let Ok(newer) = requests.try_recv() {
            request = newer;
        }
        let start = Instant::now();
        let image = match request.crop {
            Some((x, y, width, height)) => {
                let part = request.picture.image.crop_imm(x, y, width, height);
                if request.scale > 1 {
                    // Pixels are repeated, not blended: the zoom shows
                    // them as they are.
                    part.resize_exact(
                        width * request.scale,
                        height * request.scale,
                        FilterType::Nearest,
                    )
                } else {
                    part
                }
            }
            None => request.picture.image.clone(),
        };
        let resize = resize_for(request.crop.is_some());
        let protocol = picker
            .new_protocol(image, request.area, resize)
            .map_err(|err| err.to_string());
        let encoded = Encoded {
            request,
            protocol,
            elapsed: start.elapsed(),
        };
        if results.send(encoded).is_err() {
            break;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_scaled_picture_is_averaged_and_a_zoomed_one_is_not_scaled() {
        assert!(matches!(
            resize_for(false),
            Resize::Scale(Some(FilterType::Lanczos3))
        ));
        assert!(matches!(resize_for(true), Resize::Crop(None)));
    }
}
