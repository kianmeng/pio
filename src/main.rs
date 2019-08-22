// SPDX-FileCopyrightText: 2019 Tuomas Siipola
// SPDX-License-Identifier: AGPL-3.0-or-later

use clap::{App, Arg};
use dssim::{Dssim, RGBAPLU};
use imgref::{Img, ImgRef, ImgVec};
use libwebp_sys::*;
use rgb::{ComponentBytes, RGB8, RGBA, RGBA8};

use std::fs;
use std::path::Path;

type ReadResult = Result<ImgVec<RGB8>, String>;
type CompressResult = Result<(ImgVec<RGB8>, Vec<u8>), String>;

fn read_jpeg(path: impl AsRef<Path>) -> ReadResult {
    let dinfo = mozjpeg::Decompress::new_path(path).map_err(|err| err.to_string())?;
    let mut rgb = dinfo.rgb().map_err(|err| err.to_string())?;
    let width = rgb.width();
    let height = rgb.height();
    let data: Vec<RGB8> = rgb
        .read_scanlines()
        .ok_or_else(|| "Failed decode image data".to_string())?;
    rgb.finish_decompress();
    Ok(Img::new(data, width, height))
}

fn compress_jpeg(image: ImgRef<RGB8>, quality: u8) -> CompressResult {
    let mut cinfo = mozjpeg::Compress::new(mozjpeg::ColorSpace::JCS_RGB);
    cinfo.set_size(image.width(), image.height());
    cinfo.set_quality(quality as f32);
    cinfo.set_mem_dest();
    cinfo.start_compress();
    if !cinfo.write_scanlines(image.buf.as_bytes()) {
        return Err("Failed to compress image data".to_string());
    }
    cinfo.finish_compress();
    let cdata = cinfo
        .data_to_vec()
        .map_err(|_err| "Failed to compress image".to_string())?;

    let dinfo = mozjpeg::Decompress::new_mem(&cdata).map_err(|err| err.to_string())?;
    let mut rgb = dinfo.rgb().map_err(|err| err.to_string())?;
    let data: Vec<RGB8> = rgb
        .read_scanlines()
        .ok_or_else(|| "Failed to decode image data".to_string())?;
    rgb.finish_decompress();

    Ok((Img::new(data, image.width(), image.height()), cdata))
}

fn read_png(path: impl AsRef<Path>) -> ReadResult {
    let png = lodepng::decode24_file(path).map_err(|err| err.to_string())?;
    Ok(Img::new(png.buffer, png.width, png.height))
}

fn compress_png(image: ImgRef<RGB8>, quality: u8) -> CompressResult {
    let mut liq = imagequant::new();
    liq.set_quality(0, quality as u32);
    let rgba: Vec<RGBA8> = image.pixels().map(|c| c.alpha(255)).collect();
    let ref mut img = liq
        .new_image(&rgba, image.width(), image.height(), 0.0)
        .map_err(|err| err.to_string())?;
    let mut res = liq.quantize(&img).map_err(|err| err.to_string())?;
    res.set_dithering_level(1.0);
    let (palette, pixels) = res.remapped(img).map_err(|err| err.to_string())?;

    let mut state = lodepng::State::new();
    for color in &palette {
        state
            .info_raw
            .palette_add(*color)
            .map_err(|err| err.to_string())?;
        state
            .info_png
            .color
            .palette_add(*color)
            .map_err(|err| err.to_string())?;
    }
    state.info_raw.colortype = lodepng::ColorType::PALETTE;
    state.info_raw.set_bitdepth(8);
    state.info_png.color.colortype = lodepng::ColorType::PALETTE;
    state.info_png.color.set_bitdepth(8);
    state.set_auto_convert(false);
    let buffer = state
        .encode(&pixels, image.width(), image.height())
        .map_err(|err| err.to_string())?;

    let result = pixels.iter().map(|i| palette[*i as usize].rgb()).collect();

    Ok((Img::new(result, image.width(), image.height()), buffer))
}

fn read_webp(path: impl AsRef<Path>) -> ReadResult {
    let data = fs::read(path).map_err(|err| err.to_string())?;

    let mut width = 0;
    let mut height = 0;

    let ret = unsafe { WebPGetInfo(data.as_ptr(), data.len(), &mut width, &mut height) };
    if ret == 0 {
        return Err("Failed to decode file".to_string());
    }

    let len = (width * height) as usize;
    let mut buffer: Vec<RGB8> = Vec::with_capacity(len);
    unsafe {
        buffer.set_len(len);
    }

    let ret = unsafe {
        WebPDecodeRGBInto(
            data.as_ptr(),
            data.len(),
            buffer.as_mut_ptr() as *mut u8,
            (3 * width * height) as usize,
            3 * width,
        )
    };
    if ret.is_null() {
        return Err("Failed to decode image data".to_string());
    }

    Ok(Img::new(buffer, width as usize, height as usize))
}

fn compress_webp(image: ImgRef<RGB8>, quality: u8) -> CompressResult {
    unsafe {
        let mut buffer = Box::into_raw(Box::new(0u8)) as *mut _;
        let stride = image.width() as i32 * 3;
        let len = WebPEncodeRGB(
            image.buf.as_bytes().as_ptr(),
            image.width() as i32,
            image.height() as i32,
            stride,
            quality as f32,
            &mut buffer as *mut _,
        );
        if len == 0 {
            return Err("Failed to encode image data".to_string());
        }

        let capacity = image.width() * image.height();
        let mut pixels: Vec<RGB8> = Vec::with_capacity(capacity);
        pixels.set_len(capacity);

        let ret = WebPDecodeRGBInto(
            buffer,
            len,
            pixels.as_mut_ptr() as *mut u8,
            3 * image.width() * image.height(),
            (3 * image.width()) as i32,
        );
        if ret.is_null() {
            return Err("Failed to decode image data".to_string());
        }

        // XXX: Not safe because `buffer` is not allocated by `Vec`
        let buffer = Vec::from_raw_parts(buffer, len as usize, len as usize);

        Ok((Img::new(pixels, image.width(), image.height()), buffer))
    }
}

fn convert(image: ImgRef<RGB8>) -> ImgVec<RGBAPLU> {
    ImgVec::new(
        image
            .into_iter()
            .map(|x| {
                RGBA::new(
                    x.r as f32 / u8::max_value() as f32,
                    x.g as f32 / u8::max_value() as f32,
                    x.b as f32 / u8::max_value() as f32,
                    1.0,
                )
            })
            .collect(),
        image.width(),
        image.height(),
    )
}

#[derive(PartialEq)]
enum Format {
    JPEG,
    PNG,
    WEBP,
}

impl Format {
    fn detect(path: &Path) -> Option<Format> {
        path.extension()
            .and_then(std::ffi::OsStr::to_str)
            .and_then(|ext| match ext {
                "jpeg" | "jpg" => Some(Format::JPEG),
                "png" => Some(Format::PNG),
                "webp" => Some(Format::WEBP),
                _ => None,
            })
    }
}

fn compress_image(
    image: ImgRef<RGB8>,
    compressor: impl Fn(ImgRef<RGB8>, u8) -> CompressResult,
    target: f64,
    min_quality: u8,
    max_quality: u8,
    input_path: &Path,
    output_path: &Path,
) -> Result<(), String> {
    let original_size = fs::metadata(&input_path)
        .map_err(|err| err.to_string())?
        .len();
    eprintln!("original size {} bytes", original_size);

    let attr = Dssim::new();
    let original = attr
        .create_image(&convert(image))
        .ok_or_else(|| "Failed to create DSSIM image".to_string())?;

    let mut min = min_quality;
    let mut max = max_quality;
    let mut compressed;
    let mut buffer;

    loop {
        let quality = (min + max) / 2;
        let (a, b) = compressor(image, quality)?;
        compressed = a;
        buffer = b;

        let mut attr = Dssim::new();
        let (dssim, _ssim_maps) = attr.compare(
            &original,
            attr.create_image(&convert(compressed.as_ref()))
                .ok_or_else(|| "Failed create DSSIM image")?,
        );
        eprintln!(
            "range {} - {} quality {}, SSIM {:.6} {} bytes, {} % of original",
            min,
            max,
            quality,
            dssim,
            buffer.len(),
            100 * buffer.len() as u64 / original_size
        );

        if dssim > target {
            min = quality + 1;
        } else {
            max = quality - 1;
        }

        if min > max {
            break;
        }
    }

    if buffer.len() < original_size as usize {
        fs::write(output_path, buffer).map_err(|err| err.to_string())?;
    } else {
        eprintln!("Failed to optimize the input image, copying the input image to output...");
        fs::copy(input_path, output_path).map_err(|err| err.to_string())?;
    }

    Ok(())
}

fn validate_target(x: String) -> Result<(), String> {
    match x.parse::<f64>() {
        Ok(x) => {
            if x >= 0.0 {
                Ok(())
            } else {
                Err("expected value between 0.0 and infinity".to_string())
            }
        }
        Err(_) => Err("expected value between 0.0 and infinity".to_string()),
    }
}

fn validate_quality(x: String) -> Result<(), String> {
    match x.parse::<i8>() {
        Ok(x) => {
            if x >= 0 || x <= 100 {
                Ok(())
            } else {
                Err("expected value between 0 and 100".to_string())
            }
        }
        Err(_) => Err("expected value between 0 and 100".to_string()),
    }
}

fn main() {
    let matches = App::new("pio")
        .about("Perceptual Image Optimizer")
        .version(clap::crate_version!())
        .arg(
            Arg::with_name("INPUT")
                .help("Sets the input file to use")
                .required(true)
                .index(1),
        )
        .arg(
            Arg::with_name("OUTPUT")
                .help("Set the output file to use")
                .required(true)
                .index(2),
        )
        .arg(
            Arg::with_name("target")
                .long("target")
                .value_name("SSIM")
                .help("Set the target SSIM")
                .takes_value(true)
                .default_value("0.01")
                .validator(validate_target),
        )
        .arg(
            Arg::with_name("min")
                .long("min")
                .value_name("quality")
                .help("Sets the minimum quality for output")
                .takes_value(true)
                .default_value("40")
                .validator(validate_quality),
        )
        .arg(
            Arg::with_name("max")
                .long("max")
                .value_name("quality")
                .help("Sets the maximum quality for output")
                .takes_value(true)
                .default_value("95")
                .validator(validate_quality),
        )
        .get_matches();

    let input = matches.value_of("INPUT").unwrap();
    let input_path = Path::new(input);
    let input_format = match Format::detect(input_path) {
        Some(ext) => ext,
        None => {
            eprintln!("input must be jpeg, png or webp");
            std::process::exit(1);
        }
    };

    let output = matches.value_of("OUTPUT").unwrap();
    let output_path = Path::new(output);
    let output_format = match Format::detect(output_path) {
        Some(ext) => ext,
        None => {
            eprintln!("output must be jpeg, png or webp");
            std::process::exit(1);
        }
    };

    let target = matches.value_of("target").unwrap().parse().unwrap();

    let min = matches.value_of("min").unwrap().parse().unwrap();
    let max = matches.value_of("max").unwrap().parse().unwrap();
    if min > max {
        eprintln!("min must be smaller or equal to max");
        std::process::exit(1);
    }

    let input_image = match match input_format {
        Format::JPEG => read_jpeg(input_path),
        Format::PNG => read_png(input_path),
        Format::WEBP => read_webp(input_path),
    } {
        Ok(image) => image,
        Err(err) => {
            eprintln!("Failed to read input: {}", err);
            std::process::exit(1);
        }
    };

    let compressor = match output_format {
        Format::JPEG => compress_jpeg,
        Format::PNG => compress_png,
        Format::WEBP => compress_webp,
    };

    if let Err(err) = compress_image(
        input_image.as_ref(),
        compressor,
        target,
        min,
        max,
        input_path,
        output_path,
    ) {
        eprintln!("Failed to compress image: {}", err);
        std::process::exit(1);
    }
}
