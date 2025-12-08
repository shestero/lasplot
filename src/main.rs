mod config;
mod las;
mod plot;

use actix_web::{web, App, HttpRequest, HttpResponse, HttpServer, Result as ActixResult};
use actix_web::web::Bytes;
use anyhow::{Context, Result};
use base64::Engine;
use config::Config;
use futures::future::ok;
use futures::stream::once;
use futures::StreamExt;
use las::LasFile;
use plot::{hex_to_rgb, generate_plot_png, PlotConfig, RGBColor};
//use std::collections::hash_map::DefaultHasher;
//use std::hash::{Hash, Hasher};
use std::sync::Arc;

#[actix_web::main]
async fn main() -> std::io::Result<()> {
    let config = Config::load().expect("Failed to load config");
    let config = Arc::new(config);

    let bind_addr = format!("{}:{}", config.bind_address, config.bind_port);
    println!("Starting lasplot server on http://{}", bind_addr);
    
    HttpServer::new(move || {
        let config = Arc::clone(&config);
        App::new()
            .app_data(web::Data::new(config.clone()))
            .route("/", web::get().to(handle_request))
            .route("/test", web::get().to(handle_test_page))
            .route("/list", web::get().to(handle_list_files))
    })
    .bind(&bind_addr)?
    .run()
    .await
}

async fn handle_test_page(
    config: web::Data<Arc<Config>>,
) -> ActixResult<HttpResponse> {
    let samples_path = config.get_samples_path();
    let mut files = Vec::new();
    
    if let Ok(entries) = std::fs::read_dir(&samples_path) {
        for entry in entries {
            if let Ok(entry) = entry {
                let path = entry.path();
                if path.is_file() {
                    if let Some(ext) = path.extension() {
                        if ext == "las" || ext == "LAS" {
                            if let Some(name) = path.file_name() {
                                if let Some(name_str) = name.to_str() {
                                    files.push(name_str.to_string());
                                }
                            }
                        }
                    }
                }
            }
        }
    }
    
    files.sort();
    
    // Берем первые 4 файла для кнопок
    let button_files: Vec<_> = files.iter().take(4).cloned().collect();
    
    let html = format!(r#"
<!DOCTYPE html>
<html>
<head>
    <meta charset="UTF-8">
    <title>LAS Plot Test Page</title>
    <style>
        body {{
            font-family: Arial, sans-serif;
            margin: 20px;
        }}
        .button-group {{
            margin-bottom: 30px;
        }}
        .button-group button {{
            margin: 5px;
            padding: 10px 20px;
            font-size: 16px;
            cursor: pointer;
        }}
        .file-list {{
            margin-top: 20px;
        }}
        .file-list ul {{
            list-style-type: none;
            padding: 0;
        }}
        .file-list li {{
            margin: 5px 0;
        }}
        .file-list a {{
            color: #0066cc;
            text-decoration: none;
        }}
        .file-list a:hover {{
            text-decoration: underline;
        }}
    </style>
</head>
<body>
    <h1>LAS Plot Test Page</h1>
    
    <div class="button-group">
        <h2>Open in New Tabs (First 4 files):</h2>
        {}
    </div>
    
    <div class="file-list">
        <h2>All LAS Files:</h2>
        <ul id="fileList">
            {}
        </ul>
    </div>
    
    <script>
        function openFile(file) {{
            window.open('/?file=' + encodeURIComponent(file), '_blank');
        }}
        
        // Загружаем список файлов динамически
        fetch('/list')
            .then(response => response.json())
            .then(data => {{
                const list = document.getElementById('fileList');
                list.innerHTML = '';
                data.files.forEach(file => {{
                    const li = document.createElement('li');
                    const a = document.createElement('a');
                    a.href = '#';
                    a.textContent = file;
                    a.onclick = (e) => {{
                        e.preventDefault();
                        openFile(file);
                    }};
                    li.appendChild(a);
                    list.appendChild(li);
                }});
            }})
            .catch(error => {{
                console.error('Error loading file list:', error);
            }});
    </script>
</body>
</html>
"#,
        button_files.iter().map(|f| {
            format!("<button onclick=\"openFile('{}')\">{}</button>", f, f)
        }).collect::<Vec<_>>().join("\n        "),
        files.iter().map(|f| {
            format!("<li><a href=\"#\" onclick=\"openFile('{}'); return false;\">{}</a></li>", f, f)
        }).collect::<Vec<_>>().join("\n            ")
    );
    
    Ok(HttpResponse::Ok()
        .content_type("text/html; charset=utf-8")
        .body(html))
}

async fn handle_list_files(
    config: web::Data<Arc<Config>>,
) -> ActixResult<HttpResponse> {
    let samples_path = config.get_samples_path();
    let mut files = Vec::new();
    
    if let Ok(entries) = std::fs::read_dir(&samples_path) {
        for entry in entries {
            if let Ok(entry) = entry {
                let path = entry.path();
                if path.is_file() {
                    if let Some(ext) = path.extension() {
                        if ext == "las" || ext == "LAS" {
                            if let Some(name) = path.file_name() {
                                if let Some(name_str) = name.to_str() {
                                    files.push(name_str.to_string());
                                }
                            }
                        }
                    }
                }
            }
        }
    }
    
    files.sort();
    
    let json = serde_json::json!({
        "files": files
    });
    
    Ok(HttpResponse::Ok()
        .content_type("application/json")
        .body(json.to_string()))
}

async fn handle_request(
    req: HttpRequest,
    config: web::Data<Arc<Config>>,
) -> ActixResult<HttpResponse> {
    let query = req.query_string();
    let params: std::collections::HashMap<String, String> = 
        url::form_urlencoded::parse(query.as_bytes())
            .into_owned()
            .collect();

    let file_param = match params.get("file") {
        Some(file) => file,
        None => {
            // Редирект на /test если нет параметра file
            return Ok(HttpResponse::Found()
                .append_header(("Location", "/test"))
                .finish());
        }
    };

    // Получаем цвета из параметров или используем дефолтные
    let colors: Vec<String> = if let Some(colors_param) = params.get("colors") {
        colors_param.split(',').map(|s| s.trim().to_string()).collect()
    } else {
        config.default_colors.clone()
    };

    // Загружаем LAS файл
    let las_content = load_las_file(file_param, &config)
        .await
        .map_err(|e| actix_web::error::ErrorInternalServerError(format!("Failed to load LAS: {}", e)))?;

    let las_file = LasFile::parse(&las_content)
        .map_err(|e| actix_web::error::ErrorInternalServerError(format!("Failed to parse LAS: {}", e)))?;

    // Определяем основной параметр: из GET-параметра или первый параметр из LAS
    let main_param_name = params.get("main_param")
        .map(|s| s.as_str())
        .unwrap_or_else(|| {
            // По умолчанию - первый параметр из LAS файла
            las_file.curves.first()
                .map(|c| c.mnemonic.as_str())
                .unwrap_or("DEPT")
        });

    // Находим индекс основного параметра
    let main_param_idx = las_file
        .get_main_parameter_index(main_param_name)
        .ok_or_else(|| actix_web::error::ErrorInternalServerError(format!("Main parameter '{}' not found", main_param_name)))?;

    // Получаем данные глубины
    let depth_data = las_file.get_curve_data(main_param_idx);
    
    // Находим диапазон глубины
    let (depth_min, depth_max) = las_file
        .get_curve_stats(main_param_idx)
        .ok_or_else(|| actix_web::error::ErrorInternalServerError("No depth data"))?;

    // Подготавливаем данные кривых (исключаем основной параметр)
    let mut curves_data = Vec::new();
    let mut x_ranges = Vec::new();
    let mut plot_colors = Vec::new();
    let mut color_hex_strings = colors.clone();

    // Генерируем случайные цвета для недостающих параметров
    use std::collections::hash_map::DefaultHasher;
    use std::hash::{Hash, Hasher};
    for (idx, curve) in las_file.curves.iter().enumerate() {
        if idx == main_param_idx {
            continue; // Пропускаем основной параметр
        }

        let curve_data = las_file.get_curve_data(idx);
        if let Some((min, max)) = las_file.get_curve_stats(idx) {
            curves_data.push((curve_data, curve.mnemonic.as_str()));
            x_ranges.push((min, max));
            
            let color_idx = curves_data.len() - 1;
            if color_idx < color_hex_strings.len() {
                plot_colors.push(hex_to_rgb(&color_hex_strings[color_idx]));
            } else {
                // Генерируем случайный цвет на основе индекса (фиксированный для данной обработки)
                let mut hasher = DefaultHasher::new();
                (idx, &curve.mnemonic).hash(&mut hasher);
                let rng_seed = hasher.finish();
                
                // Простой генератор псевдослучайных чисел
                let r = ((rng_seed >> 0) & 0xFF) as u8;
                let g = ((rng_seed >> 8) & 0xFF) as u8;
                let b = ((rng_seed >> 16) & 0xFF) as u8;
                
                // Убеждаемся, что цвет не слишком темный
                let r = r.max(50);
                let g = g.max(50);
                let b = b.max(50);
                
                let hex_color = format!("{:02X}{:02X}{:02X}", r, g, b);
                color_hex_strings.push(hex_color.clone());
                plot_colors.push(hex_to_rgb(&hex_color));
            }
        }
    }

    if curves_data.is_empty() {
        return Err(actix_web::error::ErrorInternalServerError("No curves to plot"));
    }

    // Вычисляем количество строк
    let row_height = config.html_row_steps * config.pixels_per_step;

    // Подготавливаем данные для заголовка (клонируем нужные части)
    let curves_info: Vec<_> = las_file.curves.iter().map(|c| (c.mnemonic.clone(), c.unit.clone(), c.description.clone())).collect();
    let curves_stats: Vec<_> = (0..las_file.curves.len())
        .map(|i| las_file.get_curve_stats(i))
        .collect();

    // Создаем маппинг: индекс кривой -> hex цвет для всех кривых (кроме основного параметра)
    let mut curve_to_color: std::collections::HashMap<usize, String> = std::collections::HashMap::new();
    let mut color_idx = 0;
    for (idx, _curve) in las_file.curves.iter().enumerate() {
        if idx == main_param_idx {
            continue; // Пропускаем основной параметр
        }
        
        if color_idx < color_hex_strings.len() {
            curve_to_color.insert(idx, color_hex_strings[color_idx].clone());
            color_idx += 1;
        } else {
            // Генерируем случайный цвет для этой кривой (должно быть уже в color_hex_strings, но на всякий случай)
            let mut hasher = DefaultHasher::new();
            (idx, &las_file.curves[idx].mnemonic).hash(&mut hasher);
            let rng_seed = hasher.finish();
            
            let r = ((rng_seed >> 0) & 0xFF) as u8;
            let g = ((rng_seed >> 8) & 0xFF) as u8;
            let b = ((rng_seed >> 16) & 0xFF) as u8;
            
            let r = r.max(50);
            let g = g.max(50);
            let b = b.max(50);
            
            let hex_color = format!("{:02X}{:02X}{:02X}", r, g, b);
            curve_to_color.insert(idx, hex_color);
        }
    }

    // Генерируем HTML stream
    let stream = generate_html(
        curves_info,
        curves_stats,
        curves_data,
        x_ranges,
        plot_colors,
        depth_data,
        depth_min,
        depth_max,
        config.html_row_steps,
        config.pixels_per_step,
        row_height,
        config.image_width,
        config.separate_depth_column,
        main_param_idx,
        &curve_to_color,
    )
    .await
    .map_err(|e| actix_web::error::ErrorInternalServerError(format!("Failed to generate HTML: {}", e)))?;

    Ok(HttpResponse::Ok()
        .content_type("text/html; charset=utf-8")
        .streaming(stream))
}

async fn load_las_file(file_param: &str, config: &Config) -> Result<String> {
    if file_param.starts_with("http://") || file_param.starts_with("https://") {
        // Загружаем по URL
        let response = reqwest::get(file_param).await?;
        let content = response.text().await?;
        Ok(content)
    } else {
        // Загружаем из локальной папки
        let path = config.get_samples_path().join(file_param);
        let content = std::fs::read_to_string(&path)
            .with_context(|| format!("Failed to read file: {:?}", path))?;
        Ok(content)
    }
}

async fn generate_html_row(
    plot_config: &PlotConfig,
    curves_data: &[(Vec<Option<f64>>, &str)],
    depth_data: &[Option<f64>],
    start_block_value: usize,
    end_block_value: usize,
    row_height: usize,
    image_width: usize,
    image_height: usize,
    separate_depth_column: bool,
    depth_min: f64,
) -> Result<String> {
    let start_depth = depth_data.get(start_block_value)
        .and_then(|&d| d)
        .unwrap_or(depth_min);

    let png_data = generate_plot_png(
        plot_config,
        curves_data,
        depth_data,
        start_block_value,
        end_block_value,
    )?;

    let base64_img = base64::engine::general_purpose::STANDARD.encode(&png_data);
    
    let row_html = if separate_depth_column {
        format!(
            "<tr height='{}'><td valign='top'>{:.2}</td><td><img src='data:image/png;base64,{}' alt='Plot' width='{}' height='{}'></td></tr>\n",
            row_height, start_depth, base64_img, image_width, image_height
        )
    } else {
        format!(
            "<tr height='{}'><td><div style='position:relative'><div style='position:absolute;left:5px;top:5px'>{:.2}</div><img src='data:image/png;base64,{}' alt='Plot' width='{}' height='{}'></div></td></tr>\n",
            row_height, start_depth, base64_img, image_width, image_height
        )
    };
    
    Ok(row_html)
}

async fn generate_html(
    curves_info: Vec<(String, String, String)>,
    curves_stats: Vec<Option<(f64, f64)>>,
    curves_data: Vec<(Vec<Option<f64>>, &str)>,
    x_ranges: Vec<(f64, f64)>,
    colors: Vec<RGBColor>,
    depth_data: Vec<Option<f64>>,
    depth_min: f64,
    depth_max: f64,
    html_row_steps: usize,
    pixels_per_step: usize,
    row_height: usize,
    image_width: usize,
    separate_depth_column: bool,
    main_param_idx: usize,
    curve_to_color: &std::collections::HashMap<usize, String>,
) -> Result<impl futures::Stream<Item = std::result::Result<Bytes, actix_web::Error>>> {
    let total_steps = depth_data.len();
    let num_rows = (total_steps + html_row_steps - 1) / html_row_steps;

    // HTML над строки таблицы со шкалой
    let mut html_before_scale = String::new();
    html_before_scale.push_str("<html><head><meta charset='utf-8'><title>LAS Plot</title></head><body>\n");
    html_before_scale.push_str("<h1>LAS Plot</h1>\n");
    html_before_scale.push_str("<table border='1' cellpadding='5' style='border-collapse: collapse; border: 1px solid #ccc;'>\n");
    html_before_scale.push_str("<style>table th, table td { border: 1px solid #ccc; }</style>\n");
    html_before_scale.push_str("<tr><th>Color</th><th>Mnemonic</th><th>Measure</th><th>Description</th><th>min</th><th>max</th></tr>\n");

    for (idx, (mnemonic, unit, description)) in curves_info.iter().enumerate() {
        if let Some((min, max)) = curves_stats.get(idx).and_then(|s| *s) {
            // Определяем цвет для этой кривой
            let color_cell = if idx == main_param_idx {
                // Для основного параметра - пустая ячейка
                "<td></td>".to_string()
            } else if let Some(hex_color) = curve_to_color.get(&idx) {
                // Для остальных кривых - ячейка с цветом
                format!(
                    "<td style='background-color: #{}; color: black; text-align: center; font-weight: bold;'>{}</td>",
                    hex_color, hex_color
                )
            } else {
                // Если цвет не найден - пустая ячейка (не должно происходить)
                "<td></td>".to_string()
            };
            
            html_before_scale.push_str(&format!(
                "<tr>{}<td>{}</td><td>{}</td><td>{}</td><td>{:.2}</td><td>{:.2}</td></tr>\n",
                color_cell, mnemonic, unit, description, min, max
            ));
        }
    }

    html_before_scale.push_str("</table>\n");
    html_before_scale.push_str("<br>\n");
    html_before_scale.push_str("<table border='0' cellspacing='0' cellpadding='0'>\n");

    // HTML строки таблицы со шкалой
    // Высота шкалы соответствует полной высоте строки
    let scale_config = PlotConfig {
        width: image_width as u32,
        height: row_height as u32,
        colors: colors.clone(),
        x_ranges: x_ranges.clone(),
        y_range: (depth_min, depth_max),
        show_scales: true,
    };

    let scale_png = generate_plot_png(
        &scale_config,
        &curves_data,
        &depth_data,
        0,
        html_row_steps.min(depth_data.len()),
    )?;

    let scale_base64 = base64::engine::general_purpose::STANDARD.encode(&scale_png);
    let html_scale_row = if separate_depth_column {
        format!(
            "<tr height='{}'><td></td><td><img src='data:image/png;base64,{}' alt='Scales' width='{}' height='{}'></td></tr>\n",
            row_height, scale_base64, image_width, row_height
        )
    } else {
        format!(
            "<tr height='{}'><td><img src='data:image/png;base64,{}' alt='Scales' width='{}' height='{}'></td></tr>\n",
            row_height, scale_base64, image_width, row_height
        )
    };

    // HTML строк с изображением
    // Высота каждого блока фиксированная: html_row_steps * pixels_per_step
    let block_height = html_row_steps * pixels_per_step;
    
    let plot_config = PlotConfig {
        width: image_width as u32,
        height: block_height as u32,
        colors: colors.clone(),
        x_ranges: x_ranges.clone(),
        y_range: (depth_min, depth_max),
        show_scales: false,
    };

    let mut html_plot_rows = String::new();
    for row_idx in 0..num_rows {
        let start_block_value = row_idx * html_row_steps;
        let end_block_value = (start_block_value + html_row_steps).min(depth_data.len());

        if start_block_value >= depth_data.len() {
            break;
        }

        // Вычисляем реальное количество шагов в этом блоке для правильной высоты изображения
        let actual_steps = end_block_value - start_block_value;
        let image_height = actual_steps * pixels_per_step;

        let row_html = generate_html_row(
            &plot_config,
            &curves_data,
            &depth_data,
            start_block_value,
            end_block_value,
            block_height, // Высота строки таблицы
            image_width,
            image_height, // Высота изображения = actual_steps * pixels_per_step
            separate_depth_column,
            depth_min,
        ).await?;
        
        html_plot_rows.push_str(&row_html);
    }

    // HTML конца таблицы и документа
    let html_end = "</table>\n</body></html>\n";

    // Слепляем все части
    let html = format!("{}{}{}{}", html_before_scale, html_scale_row, html_plot_rows, html_end);
    
    // Оборачиваем в stream через once (фиктивно)
    //let stream = once(ok::<_, actix_web::Error>(Bytes::from(html)));

    let s1 = once(ok::<_, actix_web::Error>(Bytes::from(html_before_scale)));
    let s2 = once(ok::<_, actix_web::Error>(Bytes::from(html_scale_row)));
    let s3 = once(ok::<_, actix_web::Error>(Bytes::from(html_plot_rows)));
    let s4 = once(ok::<_, actix_web::Error>(Bytes::from(html_end)));

    let s = s1.chain(s2.chain(s3.chain(s4)));

    Ok(s)
}
