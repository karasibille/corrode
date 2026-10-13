//! Going through a picture one row per thread: the effects compute each
//! pixel from the input alone, so the rows are independent and every
//! core can take some. The result is the same as in one thread.

use image::{Rgb, RgbImage};
use rayon::prelude::*;

/// Bytes of a pixel.
const BYTES: usize = 3;

/// A picture whose pixel at `(x, y)` is `f(x, y)`, rows in parallel.
pub(super) fn from_fn(width: u32, height: u32, f: impl Fn(u32, u32) -> Rgb<u8> + Sync) -> RgbImage {
    let mut data = vec![0u8; width as usize * height as usize * BYTES];
    if width > 0 {
        data.par_chunks_mut(width as usize * BYTES)
            .enumerate()
            .for_each(|(y, row)| {
                for (x, pixel) in row.as_chunks_mut::<BYTES>().0.iter_mut().enumerate() {
                    *pixel = f(x as u32, y as u32).0;
                }
            });
    }
    RgbImage::from_raw(width, height, data).expect("the buffer has the size of the picture")
}

/// Changes every pixel of a picture in place with `f(x, y, pixel)`, rows
/// in parallel.
pub(super) fn for_each_pixel(image: &mut RgbImage, f: impl Fn(u32, u32, &mut Rgb<u8>) + Sync) {
    let width = image.width() as usize;
    if width == 0 {
        return;
    }
    let data: &mut [u8] = image;
    data.par_chunks_mut(width * BYTES)
        .enumerate()
        .for_each(|(y, row)| {
            for (x, bytes) in row.as_chunks_mut::<BYTES>().0.iter_mut().enumerate() {
                let mut pixel = Rgb(*bytes);
                f(x as u32, y as u32, &mut pixel);
                *bytes = pixel.0;
            }
        });
}

/// The same on the rows `top..bottom` only.
pub(super) fn for_each_pixel_in_rows(
    image: &mut RgbImage,
    top: u32,
    bottom: u32,
    f: impl Fn(u32, u32, &mut Rgb<u8>) + Sync,
) {
    let width = image.width() as usize;
    let bottom = bottom.min(image.height());
    if width == 0 || top >= bottom {
        return;
    }
    let data: &mut [u8] = image;
    let rows = &mut data[top as usize * width * BYTES..bottom as usize * width * BYTES];
    rows.par_chunks_mut(width * BYTES)
        .enumerate()
        .for_each(|(i, row)| {
            let y = top + i as u32;
            for (x, bytes) in row.as_chunks_mut::<BYTES>().0.iter_mut().enumerate() {
                let mut pixel = Rgb(*bytes);
                f(x as u32, y, &mut pixel);
                *bytes = pixel.0;
            }
        });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parallel_rows_give_the_same_picture_as_one_thread() {
        let f = |x: u32, y: u32| Rgb([(x * 3) as u8, (y * 5) as u8, (x ^ y) as u8]);
        assert_eq!(from_fn(97, 61, f), RgbImage::from_fn(97, 61, f));
        assert_eq!(from_fn(0, 5, f).dimensions(), (0, 5));

        let mut image = from_fn(97, 61, f);
        for_each_pixel(&mut image, |x, y, pixel| {
            pixel[0] = pixel[0].wrapping_add((x + y) as u8);
        });
        let mut expected = RgbImage::from_fn(97, 61, f);
        for (x, y, pixel) in expected.enumerate_pixels_mut() {
            pixel[0] = pixel[0].wrapping_add((x + y) as u8);
        }
        assert_eq!(image, expected);

        for_each_pixel_in_rows(&mut image, 10, 20, |_, _, pixel| *pixel = Rgb([1, 2, 3]));
        assert_eq!(image.get_pixel(5, 9), expected.get_pixel(5, 9));
        assert_eq!(image.get_pixel(5, 10), &Rgb([1, 2, 3]));
        assert_eq!(image.get_pixel(5, 19), &Rgb([1, 2, 3]));
        assert_eq!(image.get_pixel(5, 20), expected.get_pixel(5, 20));
    }
}
