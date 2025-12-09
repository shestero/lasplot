use anyhow::Result;
use image::{ImageEncoder, Rgba, RgbaImage};
use raqote::{
    DrawTarget, PathBuilder, Source, SolidSource, StrokeStyle, DrawOptions, LineCap, LineJoin
};

#[cfg(target_arch = "x86_64")]
use std::arch::x86_64::*;

pub type RGBColor = [u8; 3];

/// Конвертирует BGRA данные в RGBA используя SIMD-оптимизацию
/// BGRA: [B, G, R, A] -> RGBA: [R, G, B, A]
#[inline]
fn convert_bgra_to_rgba(src: &[u8], dst: &mut [u8]) {
    #[cfg(target_arch = "x86_64")]
    {
        if is_x86_feature_detected!("ssse3") {
            unsafe {
                convert_bgra_to_rgba_sse(src, dst);
                return;
            }
        }
    }
    
    // Fallback: обычное копирование с перестановкой
    convert_bgra_to_rgba_scalar(src, dst);
}

#[cfg(target_arch = "x86_64")]
#[target_feature(enable = "ssse3")]
unsafe fn convert_bgra_to_rgba_sse(src: &[u8], dst: &mut [u8]) {
    // Маска для перестановки BGRA -> RGBA используя _mm_shuffle_epi8
    // BGRA: [B=0, G=1, R=2, A=3] -> RGBA: [R=2, G=1, B=0, A=3]
    // Для каждого пикселя: индексы [2, 1, 0, 3]
    let shuffle_mask = _mm_setr_epi8(
        2, 1, 0, 3,  // Пиксель 0: BGR[A] -> RGB[A]
        6, 5, 4, 7,  // Пиксель 1
        10, 9, 8, 11, // Пиксель 2
        14, 13, 12, 15, // Пиксель 3
    );
    
    let pixel_count = src.len() / 4;
    let simd_count = pixel_count / 4; // Обрабатываем по 4 пикселя за раз (16 байт)
    let remainder = pixel_count % 4;
    
    let src_ptr = src.as_ptr();
    let dst_ptr = dst.as_mut_ptr();
    
    // Обрабатываем по 4 пикселя (16 байт) за раз
    for i in 0..simd_count {
        let offset = i * 16;
        let src_vec = _mm_loadu_si128((src_ptr.add(offset)) as *const __m128i);
        let shuffled = _mm_shuffle_epi8(src_vec, shuffle_mask);
        _mm_storeu_si128((dst_ptr.add(offset)) as *mut __m128i, shuffled);
    }
    
    // Обрабатываем оставшиеся пиксели скалярно
    if remainder > 0 {
        let offset = simd_count * 16;
        convert_bgra_to_rgba_scalar(&src[offset..], &mut dst[offset..]);
    }
}

/// Скалярная версия конвертации BGRA -> RGBA (fallback)
#[inline]
fn convert_bgra_to_rgba_scalar(src: &[u8], dst: &mut [u8]) {
    let src_pixels = src.chunks_exact(4);
    let dst_pixels = dst.chunks_exact_mut(4);
    for (src_pixel, dst_pixel) in src_pixels.zip(dst_pixels) {
        // BGRA -> RGBA
        dst_pixel[0] = src_pixel[2]; // R
        dst_pixel[1] = src_pixel[1]; // G
        dst_pixel[2] = src_pixel[0]; // B
        dst_pixel[3] = src_pixel[3]; // A
    }
}

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
    pub pixels_per_step: usize,
    pub html_row_steps: usize,
    pub scale_spacing: usize,
    pub tick_size_major: usize,
    pub tick_size_minor: usize,
}

pub fn generate_plot_png(
    config: &PlotConfig,
    curves_data: Vec<(Vec<Option<f64>>, String)>,
    depth_data: &[Option<f64>],
    depth_start_idx: usize,
    depth_end_idx: usize,
) -> Result<Vec<u8>> {
    let mut img = RgbaImage::new(config.width, config.height);
    
    // Заполняем белым фоном с непрозрачным альфа-каналом
    for pixel in img.pixels_mut() {
        *pixel = Rgba([255, 255, 255, 255]);
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
            image::ColorType::Rgba8.into(),
        )?;
    }
    
    Ok(png_data)
}

fn draw_scales(
    img: &mut RgbaImage,
    config: &PlotConfig,
    curves_data: Vec<(Vec<Option<f64>>, String)>,
) -> Result<()> {
    let plot_x_start = 100u32;
    let plot_width = (config.width as i32 - plot_x_start as i32) as u32;
    
    // Создаём DrawTarget для антиалиасинга
    let mut dt = DrawTarget::new(config.width as i32, config.height as i32);
    dt.clear(SolidSource::from_unpremultiplied_argb(0xFF, 0xFF, 0xFF, 0xFF));

    let mut y_pos = config.scale_spacing as u32;
    
    for (idx, (_, _name)) in curves_data.iter().enumerate() {
        if idx >= config.x_ranges.len() || idx >= config.colors.len() {
            continue;
        }

        let (x_min, x_max) = config.x_ranges[idx];
        let rgb = config.colors[idx];
        
        // Рисуем горизонтальную линию шкалы
        let x_start = plot_x_start as f32;
        let x_end = (plot_x_start + plot_width) as f32;
        let y = y_pos as f32;
        
        let mut pb = PathBuilder::new();
        pb.move_to(x_start, y);
        pb.line_to(x_end, y);
        let path = pb.finish();
        
        let source = Source::Solid(SolidSource {
            r: rgb[0],
            g: rgb[1],
            b: rgb[2],
            a: 255,
        });
        
        let stroke = StrokeStyle {
            width: 1.0,
            cap: LineCap::Round,
            join: LineJoin::Round,
            miter_limit: 10.0,
            ..StrokeStyle::default()
        };
        
        dt.stroke(&path, &source, &stroke, &raqote::DrawOptions::new());
        
        // Рисуем засечки только если диапазон валиден
        if x_max > x_min && x_max.is_finite() && x_min.is_finite() {
            let range = x_max - x_min;
            
            // Определяем порядок старшего разряда
            let order = range.log10().floor();
            let major_step = 10_f64.powf(order);
            let minor_step = 10_f64.powf(order - 1.0);
            
            // Находим первую длинную засечку
            let first_major = (x_min / major_step).ceil() * major_step;
            
            // Собираем позиции длинных засечек для исключения их из коротких
            let mut major_positions = std::collections::HashSet::new();
            
            // Рисуем длинные засечки
            let mut major_value = first_major;
            while major_value <= x_max {
                let t = ((major_value - x_min) / range) as f32;
                if t >= 0.0 && t <= 1.0 {
                    let tick_x = x_start + t * (x_end - x_start);
                    let tick_y_start = (y_pos.saturating_sub(config.tick_size_major as u32)) as f32;
                    let tick_y_end = (y_pos + config.tick_size_major as u32) as f32;
                    
                    let mut pb_tick = PathBuilder::new();
                    pb_tick.move_to(tick_x, tick_y_start);
                    pb_tick.line_to(tick_x, tick_y_end);
                    let path_tick = pb_tick.finish();
                    
                    dt.stroke(&path_tick, &source, &stroke, &raqote::DrawOptions::new());
                    
                    // Сохраняем позицию для исключения из коротких засечек
                    major_positions.insert((major_value / minor_step).round() as i64);
                }
                major_value += major_step;
            }
            
            // Рисуем короткие засечки
            let first_minor = (x_min / minor_step).ceil() * minor_step;
            let mut minor_value = first_minor;
            while minor_value <= x_max {
                // Пропускаем места, где уже есть длинные засечки
                let minor_index = (minor_value / minor_step).round() as i64;
                if !major_positions.contains(&minor_index) {
                    let t = ((minor_value - x_min) / range) as f32;
                    if t >= 0.0 && t <= 1.0 {
                        let tick_x = x_start + t * (x_end - x_start);
                        let tick_y_start = (y_pos.saturating_sub(config.tick_size_minor as u32)) as f32;
                        let tick_y_end = (y_pos + config.tick_size_minor as u32) as f32;
                        
                        let mut pb_tick = PathBuilder::new();
                        pb_tick.move_to(tick_x, tick_y_start);
                        pb_tick.line_to(tick_x, tick_y_end);
                        let path_tick = pb_tick.finish();
                        
                        dt.stroke(&path_tick, &source, &stroke, &raqote::DrawOptions::new());
                    }
                }
                minor_value += minor_step;
            }
        }

        y_pos += config.scale_spacing as u32;
        if y_pos >= config.height {
            break;
        }
    }
    
    // Копируем из DrawTarget (BGRA) в RgbaImage (RGBA) с SIMD-оптимизацией
    let data_u8: &[u8] = dt.get_data_u8();
    let dst = img.as_mut();
    
    convert_bgra_to_rgba(data_u8, dst);

    Ok(())
}

fn draw_curves(
    img: &mut RgbaImage,
    config: &PlotConfig,
    curves_data: Vec<(Vec<Option<f64>>, String)>,
    depth_data: &[Option<f64>],
    depth_start_idx: usize,
    depth_end_idx: usize,
) -> Result<()> {
    let plot_width = config.width as f64;
    let plot_height = config.height as f64; // (config.height as f64 * 1.04) as f64; // TODO: coef!
    let plot_x_start = 100 as f64;
    let plot_y_start = 0 as f64;
    let (mut y_min, mut y_max) = config.y_range;

    // Собираем данные для текущего диапазона
    let mut depth_slice = Vec::new();
    let mut valid_indices = Vec::new();

    let (mut ymin, mut ymax): (Option::<f64>, Option::<f64>) = (None, None);
    // depth_end_idx может быть на 1 больше для включения общего шага со следующей строкой
    // Включаем все индексы от depth_start_idx до depth_end_idx (включительно)
    // Но не выходим за границы массива
    let actual_end = depth_end_idx.min(depth_data.len());
    // Используем ..= для включения последнего индекса, если он в пределах массива
    for i in depth_start_idx..actual_end {
        if let Some(depth) = depth_data.get(i).and_then(|&d| d) {
            ymin = Some(ymin.map_or(depth, |m| m.min(depth)));
            ymax = Some(ymax.map_or(depth, |m| m.max(depth)));
            depth_slice.push(depth);
            valid_indices.push(i);
        }
    }
    // Если depth_end_idx указывает на следующий шаг (html_row_steps+1) и он в пределах массива, включаем его
    // Это нужно для того, чтобы последний шаг был общим со следующей строкой
    if depth_end_idx < depth_data.len() && depth_end_idx >= actual_end {
        if let Some(depth) = depth_data.get(depth_end_idx).and_then(|&d| d) {
            ymin = Some(ymin.map_or(depth, |m| m.min(depth)));
            ymax = Some(ymax.map_or(depth, |m| m.max(depth)));
            depth_slice.push(depth);
            valid_indices.push(depth_end_idx);
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

        // Обрабатываем все индексы из valid_indices (включая последний шаг html_row_steps+1, если он есть)
        for (slice_idx, &data_idx) in valid_indices.iter().enumerate() {
            if data_idx >= data.len() {
                continue;
            }

            if let Some(value) = data[data_idx] {
                let depth = depth_slice[slice_idx];
                
                let x = plot_x_start + ((value - x_min) / (x_max - x_min)) * plot_width;
                // Исправляем формулу: y_min (меньшая глубина) должна быть вверху (y=0), y_max (большая глубина) - внизу (y=height)
                let y = plot_y_start + ((depth - y_min) / (y_max - y_min)) * plot_height;

                let x_int = x as u32;
                let y_int = y as u32;

                if x_int < config.width && y_int < config.height {
                    // Рисуем линию от предыдущей точки
                    if let Some((last_x, last_y)) = last_point {
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
    // Мы прочитаем их и конвертируем в image::RgbaImage (RGBA).
    let data_u8: &[u8] = dt.get_data_u8(); // &[u8], порядок BGRA для каждого пикселя
    // data_u8.len() == (w*h*4)

    // Копируем в RgbaImage (RGBA) с SIMD-оптимизацией
    // Raqote: BGRA per-pixel (b,g,r,a) on little-endian. Конвертируем в RGBA.
    let dst = img.as_mut();
    // dst.len() == w*h*4
    
    convert_bgra_to_rgba(data_u8, dst);
    
    Ok(())
}

// todo: delete
fn draw_line(img: &mut RgbaImage, x1: u32, y1: u32, x2: u32, y2: u32, color: [u8; 3]) {
    let dx = (x2 as i32 - x1 as i32).abs();
    let dy = (y2 as i32 - y1 as i32).abs();
    let sx = if x1 < x2 { 1 } else { -1 };
    let sy = if y1 < y2 { 1 } else { -1 };
    let mut err = dx - dy;
    let mut x = x1 as i32;
    let mut y = y1 as i32;

    loop {
        if x >= 0 && x < img.width() as i32 && y >= 0 && y < img.height() as i32 {
            img.put_pixel(x as u32, y as u32, Rgba([color[0], color[1], color[2], 255]));
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

/// Рисует антиалиасную линию в DrawTarget
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
    img: &mut RgbaImage,
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
    // Мы прочитаем их и конвертируем в image::RgbaImage (RGBA).
    let data_u8 = dt.get_data_u8(); // &[u8], порядок BGRA для каждого пикселя
    // data_u8.len() == (w*h*4)

    // Копируем в RgbaImage (RGBA) с SIMD-оптимизацией
    // Raqote: BGRA per-pixel (b,g,r,a) on little-endian. Конвертируем в RGBA.
    let dst = img.as_mut();
    // dst.len() == w*h*4
    
    convert_bgra_to_rgba(data_u8, dst);
}