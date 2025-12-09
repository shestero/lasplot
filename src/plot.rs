use anyhow::Result;
use image::{ImageEncoder, Rgb, RgbImage};
use raqote::{
    DrawTarget, PathBuilder, Source, SolidSource, StrokeStyle, DrawOptions, LineCap, LineJoin
};

pub type RGBColor = [u8; 3];

pub fn hex_to_rgb(hex: &str) -> RGBColor {
    let hex = hex.trim_start_matches('#');
    if hex.len() >= 6 {
        let r = u8::from_str_radix(&hex[0..2], 16).unwrap_or(0);
        let g = u8::from_str_radix(&hex[2..4], 16).unwrap_or(0);
        let b = u8::from_str_radix(&hex[4..6], 16).unwrap_or(0);
        [r, g, b]
    } else {
        [0, 0, 0]
    }
}

pub struct PlotConfig {
    pub width: u32,
    pub height: u32,
    pub colors: Vec<RGBColor>,
    pub x_ranges: Vec<(f64, f64)>,
    pub y_range: (f64, f64),
    pub show_scales: bool,
}

pub fn generate_plot_png(
    config: &PlotConfig,
    curves_data: Vec<(Vec<Option<f64>>, String)>,
    depth_data: &[Option<f64>],
    depth_start_idx: usize,
    depth_end_idx: usize,
) -> Result<Vec<u8>> {
    let mut img = RgbImage::new(config.width, config.height);
    
    // Заполняем белым фоном
    for pixel in img.pixels_mut() {
        *pixel = Rgb([255, 255, 255]);
    }

    if config.show_scales {
        // Рисуем шкалы для каждого параметра
        draw_scales(&mut img, config, curves_data)?;
    } else {
        // Рисуем графики
        draw_curves(&mut img, config, curves_data, depth_data, depth_start_idx, depth_end_idx)?;
    }

    // Конвертируем в PNG
    let mut png_data = Vec::new();
    {
        let encoder = image::codecs::png::PngEncoder::new(&mut png_data);
        encoder.write_image(
            &img.into_raw(),
            config.width,
            config.height,
            image::ColorType::Rgb8.into(),
        )?;
    }
    
    Ok(png_data)
}

fn draw_scales(
    img: &mut RgbImage,
    config: &PlotConfig,
    curves_data: Vec<(Vec<Option<f64>>, String)>,
) -> Result<()> {
    let mut x_pos = 20u32;
    let scale_height = (config.height as f64 * 0.8) as u32;
    let scale_y_start = (config.height as f64 * 0.1) as u32;

    for (idx, (_, _name)) in curves_data.iter().enumerate() {
        if idx >= config.x_ranges.len() {
            continue;
        }

        let (_min, _max) = config.x_ranges[idx];
        
        // Рисуем вертикальную линию шкалы
        for y in scale_y_start..(scale_y_start + scale_height) {
            if x_pos < config.width {
                img.put_pixel(x_pos, y, Rgb([0, 0, 0]));
            }
        }

        // Рисуем метки
        let num_ticks = 5;
        for i in 0..=num_ticks {
            let tick_y = scale_y_start + (i * scale_height / num_ticks);
            
            // Короткая горизонтальная линия
            for x in x_pos.saturating_sub(5)..=x_pos.saturating_add(5) {
                if x < config.width && tick_y < config.height {
                    img.put_pixel(x, tick_y, Rgb([0, 0, 0]));
                }
            }
        }

        x_pos += 80;
        if x_pos >= config.width {
            break;
        }
    }

    Ok(())
}

fn draw_curves(
    img: &mut RgbImage,
    config: &PlotConfig,
    curves_data: Vec<(Vec<Option<f64>>, String)>,
    depth_data: &[Option<f64>],
    depth_start_idx: usize,
    depth_end_idx: usize,
) -> Result<()> {
    let plot_width = config.width as f64;
    let plot_height = config.height as f64;
    let plot_x_start = 100 as f64;
    let plot_y_start = 0 as f64;
    let (mut y_min, mut y_max) = config.y_range;

    // Собираем данные для текущего диапазона
    let mut depth_slice = Vec::new();
    let mut valid_indices = Vec::new();

    let (mut ymin, mut ymax): (Option::<f64>, Option::<f64>) = (None, None);
    for i in depth_start_idx..depth_end_idx.min(depth_data.len()) {
        if let Some(depth) = depth_data.get(i).and_then(|&d| d) {
            ymin = Some(ymin.map_or(depth, |m| m.min(depth)));
            ymax = Some(ymax.map_or(depth, |m| m.max(depth)));
            depth_slice.push(depth);
            valid_indices.push(i);
        }
    }
    // avoid div to zero
    y_min = ymin.unwrap_or(y_min);
    y_max = ymax.unwrap_or(y_max);

    if depth_slice.is_empty() {
        return Ok(());
    }

    // Создаём DrawTarget того же размера (ARGB backing)
    let mut dt = DrawTarget::new(config.width as i32, config.height as i32);

    // (Опционально?) очистим фон прозрачным/белым
    dt.clear(SolidSource::from_unpremultiplied_argb(0xFF, 0xFF, 0xFF, 0xFF));

    // Рисуем графики в обратном порядке (первые параметры важнее, не перекрываются)
    for (curve_idx, (data, _)) in curves_data.iter().enumerate().rev() {
        if curve_idx >= config.x_ranges.len() || curve_idx >= config.colors.len() {
            continue;
        }

        let (x_min, x_max) = config.x_ranges[curve_idx];
            let rgb = config.colors[curve_idx];

        let mut last_point: Option<(u32, u32)> = None;

        for (slice_idx, &data_idx) in valid_indices.iter().enumerate() {
            if data_idx >= data.len() {
                continue;
            }

            if let Some(value) = data[data_idx] {
                let depth = depth_slice[slice_idx];
                
                let x = plot_x_start + ((value - x_min) / (x_max - x_min)) * plot_width;
                let y = plot_y_start + ((depth - y_max) / (y_min - y_max)) * plot_height;

                let x_int = x as u32;
                let y_int = y as u32;

                if x_int < config.width && y_int < config.height {
                    // Рисуем точку
                    //img.put_pixel(x_int, y_int, Rgb(rgb));
                    
                    // Рисуем линию от предыдущей точки
                    if let Some((last_x, last_y)) = last_point {
                        //draw_line(img, last_x, last_y, x_int, y_int, rgb);
                        draw_line_dt(&mut dt, last_x, last_y, x_int, y_int, rgb);
                    }

                    last_point = Some((x_int, y_int));
                }
            } else {
                // NaN или null - разрыв линии
                last_point = None;
            }
        }
    }

    // Получаем сырые байты BGRA (u8) из DrawTarget
    // docs.rs: get_data_u8() / get_data_u8_mut() дают BGRA порядок (little endian).
    // Мы прочитаем их и конвертируем в image::RgbImage (RGB).
    let data_u8: &[u8] = dt.get_data_u8(); // &[u8], порядок BGRA для каждого пикселя
    // data_u8.len() == (w*h*4)

    // Копируем в RgbImage (RGB)
    // Raqote: BGRA per-pixel (b,g,r,a) on little-endian. Берём b,g,r и игнорируем alpha.
    let mut dst = img.as_mut(); // ??
    // dst.len() == w*h*3

    for y in 0..config.height {
        for x in 0..config.width {
            let src_idx: usize = ((y * config.width + x) * 4).try_into().unwrap();
            let b = data_u8[src_idx + 0];
            let g = data_u8[src_idx + 1];
            let r = data_u8[src_idx + 2];
            let _a = data_u8[src_idx + 3];

            let dst_idx: usize = ((y * config.width + x) * 3).try_into().unwrap();
            dst[dst_idx + 0] = r;
            dst[dst_idx + 1] = g;
            dst[dst_idx + 2] = b;
        }
    }
    Ok(())
}

// todo: delete
fn draw_line(img: &mut RgbImage, x1: u32, y1: u32, x2: u32, y2: u32, color: [u8; 3]) {
    let dx = (x2 as i32 - x1 as i32).abs();
    let dy = (y2 as i32 - y1 as i32).abs();
    let sx = if x1 < x2 { 1 } else { -1 };
    let sy = if y1 < y2 { 1 } else { -1 };
    let mut err = dx - dy;
    let mut x = x1 as i32;
    let mut y = y1 as i32;

    loop {
        if x >= 0 && x < img.width() as i32 && y >= 0 && y < img.height() as i32 {
            img.put_pixel(x as u32, y as u32, Rgb(color));
        }

        if x == x2 as i32 && y == y2 as i32 {
            break;
        }

        let e2 = 2 * err;
        if e2 > -dy {
            err -= dy;
            x += sx;
        }
        if e2 < dx {
            err += dx;
            y += sy;
        }
    }
}

/// Рисует антиалиасную линию в `RgbImage`
/// расстояния — в пикселях (u32)
pub fn draw_line_dt(
    dt: &mut DrawTarget,
    x1: u32,
    y1: u32,
    x2: u32,
    y2: u32,
    color: [u8; 3],
) {
    // Векторный путь (одна линия)
    let mut pb = PathBuilder::new();
    pb.move_to(x1 as f32, y1 as f32);
    pb.line_to(x2 as f32, y2 as f32);
    let path = pb.finish();

    // Цвет
    let source = Source::Solid(SolidSource {
        r: color[0],
        g: color[1],
        b: color[2],
        a: 255,
    });

    // Параметры обводки
    let stroke = StrokeStyle {
        width: 1.0,                 // толщина линии
        cap: LineCap::Round,        // округлые окончания
        join: LineJoin::Round,      // сглаженные углы
        miter_limit: 10.0,
        ..StrokeStyle::default()
    };

    dt.stroke(&path, &source, &stroke, &raqote::DrawOptions::new());
}

// todo: delete
pub fn draw_line_new(
    img: &mut RgbImage,
    x1: u32,
    y1: u32,
    x2: u32,
    y2: u32,
    color: [u8; 3],
) {
    let w = img.width() as i32;
    let h = img.height() as i32;

    // Создаём DrawTarget того же размера (ARGB backing)
    let mut dt = DrawTarget::new(w as i32, h as i32);

    // (Опционально) очистим фон прозрачным/белым
    dt.clear(SolidSource::from_unpremultiplied_argb(0xFF, 0xFF, 0xFF, 0xFF));

    // Построим путь: простая линия от (x1,y1) -> (x2,y2)
    let mut pb = PathBuilder::new();
    pb.move_to(x1 as f32, y1 as f32);
    pb.line_to(x2 as f32, y2 as f32);
    let path = pb.finish();

    // Цвет (raqote использует premultiplied ARGB helpers)
    let src = Source::Solid(SolidSource::from_unpremultiplied_argb(
        0xFF, color[0], color[1], color[2],
    ));

    // Параметры обводки (толщина линии в пикселях)
    let stroke_style = StrokeStyle {
        width: 1.5, // меняй толщину
        ..StrokeStyle::default()
    };

    // Рендерим stroke
    dt.stroke(&path, &src, &stroke_style, &DrawOptions::new());

    // Получаем сырые байты BGRA (u8) из DrawTarget
    // docs.rs: get_data_u8() / get_data_u8_mut() дают BGRA порядок (little endian).
    // Мы прочитаем их и конвертируем в image::RgbImage (RGB).
    let data_u8 = dt.get_data_u8(); // &[u8], порядок BGRA для каждого пикселя
    // data_u8.len() == (w*h*4)

    // Копируем в RgbImage (RGB)
    // Raqote: BGRA per-pixel (b,g,r,a) on little-endian. Берём b,g,r и игнорируем alpha.
    let mut dst = img.as_mut();
    // dst.len() == w*h*3
    let width = w as usize;
    let height = h as usize;

    for y in 0..height {
        for x in 0..width {
            let src_idx = (y * width + x) * 4;
            let b = data_u8[src_idx + 0];
            let g = data_u8[src_idx + 1];
            let r = data_u8[src_idx + 2];
            // let a = data_u8[src_idx + 3];

            let dst_idx = (y * width + x) * 3;
            dst[dst_idx + 0] = r;
            dst[dst_idx + 1] = g;
            dst[dst_idx + 2] = b;
        }
    }
}