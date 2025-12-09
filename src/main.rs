mod config;
mod las;
mod plot;

use actix_web::{web, App, HttpRequest, HttpResponse, HttpServer, Result as ActixResult};
use actix_web::web::Bytes;
use anyhow::{Context, Result};
use base64::Engine;
use config::Config;
use futures::future::ok;
use futures::stream::{self, once, StreamExt};
use futures::TryStreamExt;
use las::LasFile;
use plot::{hex_to_rgb, generate_plot_png, PlotConfig, RGBColor};
//use std::collections::hash_map::DefaultHasher;
//use std::hash::{Hash, Hasher};
use std::sync::Arc;
use url::Url;

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

fn get_files_from_samples(config: &Config) -> Vec<String> {
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
    files
}

#[derive(Debug, Clone)]
struct LasFileInfo {
    url: String,
    operator: Option<String>,
    lease: Option<String>,
    depth_start: Option<String>,
    depth_stop: Option<String>,
}

fn parse_csv_line(line: &str) -> Vec<String> {
    let mut fields = Vec::new();
    let mut current = String::new();
    let mut in_quotes = false;
    
    for ch in line.chars() {
        match ch {
            '"' => {
                in_quotes = !in_quotes;
            }
            ',' if !in_quotes => {
                fields.push(current.trim_matches('"').to_string());
                current.clear();
            }
            _ => {
                current.push(ch);
            }
        }
    }
    if !current.is_empty() {
        fields.push(current.trim_matches('"').to_string());
    }
    fields
}

fn is_csv_format(content: &str) -> bool {
    // Проверяем, является ли файл CSV форматом
    // CSV формат обычно начинается с заголовка в кавычках
    let first_line = content.lines().next().unwrap_or("").trim();
    first_line.starts_with('"') && first_line.contains(',')
}

fn read_laslist_file(config: &Config) -> Result<Vec<String>> {
    let laslist_path = &config.laslist_file;
    
    // Если файл не существует, используем список из samples
    if !std::path::Path::new(laslist_path).exists() {
        return Ok(get_files_from_samples(config));
    }
    
    let content = std::fs::read_to_string(laslist_path)
        .context("Failed to read laslist.txt")?;
    
    // Определяем формат файла
    if is_csv_format(&content) {
        // CSV формат - возвращаем пустой список, так как нужна дополнительная информация
        return Ok(Vec::new());
    }
    
    // Простой формат - список файлов
    let files: Vec<String> = content
        .lines()
        .map(|s| s.trim())
        .filter(|s| {
            // Игнорируем пустые строки и комментарии
            !s.is_empty() && !s.starts_with('#')
        })
        .map(|s| s.to_string())
        .collect();
    
    Ok(files)
}

fn read_laslist_file_with_info(config: &Config) -> Result<Vec<LasFileInfo>> {
    // Используем laslist_file из конфигурации, или по умолчанию "lasfiles.txt"
    let laslist_path = if config.laslist_file.is_empty() {
        "lasfiles.txt"
    } else {
        &config.laslist_file
    };
    
    // Если файл не существует, используем список из samples
    if !std::path::Path::new(laslist_path).exists() {
        let files = get_files_from_samples(config);
        return Ok(files.into_iter().map(|url| LasFileInfo {
            url,
            operator: None,
            lease: None,
            depth_start: None,
            depth_stop: None,
        }).collect());
    }
    
    let content = std::fs::read_to_string(laslist_path)
        .context("Failed to read laslist.txt")?;
    
    // Определяем формат файла
    if is_csv_format(&content) {
        // CSV формат
        let lines: Vec<&str> = content.lines().collect();
        if lines.is_empty() {
            return Ok(Vec::new());
        }
        
        // Парсим заголовок
        let header = parse_csv_line(lines[0]);
        let url_idx = header.iter().position(|s| s == "URL").unwrap_or(usize::MAX);
        let operator_idx = header.iter().position(|s| s == "Operator").unwrap_or(usize::MAX);
        let lease_idx = header.iter().position(|s| s == "Lease").unwrap_or(usize::MAX);
        let depth_start_idx = header.iter().position(|s| s == "Depth_start").unwrap_or(usize::MAX);
        let depth_stop_idx = header.iter().position(|s| s == "Depth_stop").unwrap_or(usize::MAX);
        
        if url_idx == usize::MAX {
            return Ok(Vec::new());
        }
        
        let mut files_info = Vec::new();
        for line in lines.iter().skip(1) {
            let trimmed = line.trim();
            if trimmed.is_empty() || trimmed.starts_with('#') {
                continue;
            }
            
            let fields = parse_csv_line(trimmed);
            if url_idx < fields.len() {
                let url = fields[url_idx].clone();
                if !url.is_empty() {
                    files_info.push(LasFileInfo {
                        url,
                        operator: if operator_idx < fields.len() && !fields[operator_idx].is_empty() {
                            Some(fields[operator_idx].clone())
                        } else {
                            None
                        },
                        lease: if lease_idx < fields.len() && !fields[lease_idx].is_empty() {
                            Some(fields[lease_idx].clone())
                        } else {
                            None
                        },
                        depth_start: if depth_start_idx < fields.len() && !fields[depth_start_idx].is_empty() {
                            Some(fields[depth_start_idx].clone())
                        } else {
                            None
                        },
                        depth_stop: if depth_stop_idx < fields.len() && !fields[depth_stop_idx].is_empty() {
                            Some(fields[depth_stop_idx].clone())
                        } else {
                            None
                        },
                    });
                }
            }
        }
        
        Ok(files_info)
    } else {
        // Простой формат - список файлов
        let files: Vec<String> = content
            .lines()
            .map(|s| s.trim())
            .filter(|s| {
                // Игнорируем пустые строки и комментарии
                !s.is_empty() && !s.starts_with('#')
            })
            .map(|s| s.to_string())
            .collect();
        
        Ok(files.into_iter().map(|url| LasFileInfo {
            url,
            operator: None,
            lease: None,
            depth_start: None,
            depth_stop: None,
        }).collect())
    }
}

fn is_url(s: &str) -> bool {
    s.starts_with("http://") || s.starts_with("https://")
}

fn get_server_name(url_str: &str) -> String {
    if let Ok(url) = Url::parse(url_str) {
        if let Some(host) = url.host_str() {
            return host.to_string();
        }
    }
    "unknown".to_string()
}

async fn get_las_version(file_path: &str, config: &Config) -> Option<String> {
    let content = load_las_file(file_path, config).await.ok()?;
    let las_file = LasFile::parse(&content).ok()?;
    Some(las_file.version)
}

async fn handle_test_page(
    config: web::Data<Arc<Config>>,
) -> ActixResult<HttpResponse> {
    let files_info = read_laslist_file_with_info(&config)
        .map_err(|e| actix_web::error::ErrorInternalServerError(format!("Failed to read laslist: {}", e)))?;
    
    // Берем первые 4 файла для кнопок
    let button_files: Vec<_> = files_info.iter().take(4).map(|f| f.url.as_str()).collect();
    
    // Создаем список файлов с информацией
    let mut file_items = Vec::new();
    for file_info in &files_info {
        let url = &file_info.url;
        let mut extra_info = Vec::new();
        
        // Добавляем информацию об Operator и Lease
        if let Some(ref operator) = file_info.operator {
            extra_info.push(format!("Operator: {}", operator));
        }
        if let Some(ref lease) = file_info.lease {
            extra_info.push(format!("Lease: {}", lease));
        }
        
        // Добавляем диапазон глубины
        if let (Some(ref start), Some(ref stop)) = (&file_info.depth_start, &file_info.depth_stop) {
            extra_info.push(format!("Depth: {} - {}", start, stop));
        }
        
        let extra_info_str = if !extra_info.is_empty() {
            format!("<br><span style='font-size: 0.85em; color: #666; margin-left: 20px;'>{}</span>", extra_info.join(", "))
        } else {
            String::new()
        };
        
        if is_url(url) {
            let server_name = get_server_name(url);
            file_items.push(format!(
                "<li><a href=\"#\" onclick=\"openFile('{}'); return false;\">{}</a> <span style='font-size: 0.8em; color: #666;'>({})</span>{}</li>",
                url, url, server_name, extra_info_str
            ));
        } else {
            // Локальный файл - получаем версию LAS
            let version_info = if let Some(version) = get_las_version(url, &config).await {
                format!(" <span style='font-size: 0.8em; color: #666;'>(LAS {})</span>", version)
            } else {
                String::new()
            };
            file_items.push(format!(
                "<li><a href=\"#\" onclick=\"openFile('{}'); return false;\">{}</a>{}{}</li>",
                url, url, version_info, extra_info_str
            ));
        }
    }
    
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
        <ul>
            {}
        </ul>
    </div>
    
    <script>
        function openFile(file) {{
            window.open('/?file=' + encodeURIComponent(file), '_blank');
        }}
    </script>
</body>
</html>
"#,
        button_files.iter().map(|f| {
            format!("<button onclick=\"openFile('{}')\">{}</button>", f, f)
        }).collect::<Vec<_>>().join("\n        "),
        file_items.join("\n            ")
    );
    
    Ok(HttpResponse::Ok()
        .content_type("text/html; charset=utf-8")
        .body(html))
}

async fn handle_list_files(
    config: web::Data<Arc<Config>>,
) -> ActixResult<HttpResponse> {
    let files_info = read_laslist_file_with_info(&config)
        .map_err(|e| actix_web::error::ErrorInternalServerError(format!("Failed to read laslist: {}", e)))?;
    
    let files: Vec<String> = files_info.iter().map(|f| f.url.clone()).collect();
    
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
    let curves = Arc::new(las_file.curves.clone());
    for (idx, curve) in curves.iter().enumerate() {
        if idx == main_param_idx {
            continue; // Пропускаем основной параметр
        }

        let curve_data = las_file.get_curve_data(idx);
        if let Some((min, max)) = las_file.get_curve_stats(idx) {
            curves_data.push((curve_data, curve.mnemonic.clone()));
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
    // Высота = html_row_steps * pixels_per_step + 1
    let row_height = config.html_row_steps * config.pixels_per_step + 1;

    // Подготавливаем данные для заголовка (клонируем нужные части)
    let curves_info: Vec<_> = curves.iter().map(|c| (c.mnemonic.clone(), c.unit.clone(), c.description.clone())).collect();
    let curves_stats: Vec<_> = (0..curves.len())
        .map(|i| las_file.get_curve_stats(i))
        .collect();

    // Создаем маппинг: индекс кривой -> hex цвет для всех кривых (кроме основного параметра)
    let mut curve_to_color: std::collections::HashMap<usize, String> = std::collections::HashMap::new();
    let mut color_idx = 0;
    for (idx, _curve) in curves.iter().enumerate() {
        if idx == main_param_idx {
            continue; // Пропускаем основной параметр
        }
        
        if color_idx < color_hex_strings.len() {
            curve_to_color.insert(idx, color_hex_strings[color_idx].clone());
            color_idx += 1;
        } else {
            // Генерируем случайный цвет для этой кривой (должно быть уже в color_hex_strings, но на всякий случай)
            let mut hasher = DefaultHasher::new();
            (idx, &curves[idx].mnemonic).hash(&mut hasher);
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

    // Извлекаем информацию из секции ~Well
    // Формат: (ключ, описание) - описание используется как заголовок
    let well_info_keys = vec![
        ("COMP", "COMPANY"),
        ("WELL", "WELL"),
        ("FLD", "FIELD"),
        ("LOC", "LOCATION"),
        ("SRVC", "SERVICE COMPANY"),
        ("DATE", "LOG DATE"),
        ("PROV", "PROVINCE"),
    ];
    
    let well_info_text: Vec<(String, String)> = well_info_keys
        .iter()
        .filter_map(|(key, description)| {
            // Пробуем найти ключ с точкой и без точки
            las_file.well_info.get(*key)
                .or_else(|| las_file.well_info.get(&format!("{}.", key)))
                .map(|value| (description.to_string(), value.clone()))
        })
        .collect();

    // Генерируем HTML stream
    let stream = generate_html(
        curves_info,
        curves_stats,
        curves_data.into(),
        x_ranges,
        plot_colors,
        depth_data,//.into(),
        depth_min,
        depth_max,
        config.html_row_steps,
        config.pixels_per_step,
        row_height,
        config.image_width,
        config.separate_depth_column,
        main_param_idx,
        curve_to_color,
        file_param,
        &well_info_text,
        config.scale_spacing,
        config.max_scales,
    ).map_err(|e| actix_web::error::ErrorInternalServerError(e))
    .map_err(|e| actix_web::error::ErrorInternalServerError(format!("Failed to generate HTML: {}", e)))?;

    let response =
        HttpResponse::Ok()
            .content_type("text/html; charset=utf-8")
            .streaming(stream);
    Ok(response)
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
    curves_data: Vec<(Vec<Option<f64>>, String)>,
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
        curves_data.into(),
        depth_data,
        start_block_value,
        end_block_value,
    )?;

    let base64_img = base64::engine::general_purpose::STANDARD.encode(&png_data);
    
    let row_html = if separate_depth_column {
        format!(
            "<tr height='{}' style='vertical-align: top; margin: 0; padding: 0;'><td valign='top' style='padding: 0; margin: 0; border: 1px solid #ccc;'>{:.2}</td><td style='padding: 0; margin: 0; border: 1px solid #ccc; vertical-align: top;'><img src='data:image/png;base64,{}' alt='Plot' width='{}' height='{}' style='display: block; margin: 0; padding: 0;'></td></tr>\n",
            row_height, start_depth, base64_img, image_width, image_height
        )
    } else {
        format!(
            "<tr height='{}' style='vertical-align: top; margin: 0; padding: 0;'><td style='padding: 0; margin: 0; border: 1px solid #ccc; vertical-align: top;'><div style='position:relative; margin: 0; padding: 0;'><div style='position:absolute;left:5px;top:5px'>{:.2}</div><img src='data:image/png;base64,{}' alt='Plot' width='{}' height='{}' style='display: block; margin: 0; padding: 0;'></div></td></tr>\n",
            row_height, start_depth, base64_img, image_width, image_height
        )
    };
    
    Ok(row_html)
}

fn generate_html(
    curves_info: Vec<(String, String, String)>,
    curves_stats: Vec<Option<(f64, f64)>>,
    curves_data: Arc<Vec<(Vec<Option<f64>>, String)>>,
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
    curve_to_color: std::collections::HashMap<usize, String>,
    file_name: &str,
    well_info: &[(String, String)],
    scale_spacing: usize,
    max_scales: usize,
) -> Result<impl futures::Stream<Item = Result<Bytes, actix_web::Error>> + 'static, actix_web::Error> {
    //let curves_data = Arc::new(curves_data);
    let depth_data = Arc::new(depth_data);

    let total_steps = depth_data.len();
    let num_rows = (total_steps + html_row_steps - 1) / html_row_steps;

    // HTML над строки таблицы со шкалой
    let mut html_before_scale = String::new();
    html_before_scale.push_str(&format!("<html><head><meta charset='utf-8'><title>LAS Plot - {}</title></head><body>\n", file_name));
    html_before_scale.push_str(&format!("<h2>LAS Plot - {}</h2>\n", file_name));
    
    // Генерируем HTML для двух верхних таблиц
    let mut curves_table_html = String::new();
    curves_table_html.push_str("<table border='1' cellpadding='5' style='border-collapse: collapse; border: 1px solid #ccc; font-family: monospace;'>\n");
    curves_table_html.push_str("<style>table th, table td { border: 1px solid #ccc; }</style>\n");
    curves_table_html.push_str("<tr><th>Color</th><th>Mnemonic</th><th>Measure</th><th>Description</th><th>min</th><th>max</th></tr>\n");

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
            
            curves_table_html.push_str(&format!(
                "<tr>{}<td>{}</td><td>{}</td><td>{}</td><td style='text-align: right;'>{:.2}</td><td style='text-align: right;'>{:.2}</td></tr>\n",
                color_cell, mnemonic, unit, description, min, max
            ));
        }
    }
    curves_table_html.push_str("</table>\n");
    
    let mut well_table_html = String::new();
    // Выводим информацию из секции ~Well в таблице с 2 колонками
    if !well_info.is_empty() {
        well_table_html.push_str("<table style='border: none; border-style: none; border-collapse: collapse; font-family: monospace; border-spacing: 0;'>\n");
        for (key, value) in well_info {
            // Разделяем на часть до двоеточия и после
            if let Some(colon_pos) = value.find(':') {
                let before_colon = &value[..colon_pos].trim(); // Без двоеточия
                let after_colon = &value[colon_pos + 1..].trim();
                well_table_html.push_str(&format!(
                    "<tr style='border: none'><td style='text-align: right; padding-right: 5px; border: none'>{}{}</td><td style='text-align: left; padding-left: 5px; border: none'>{}</td></tr>\n",
                    before_colon, ":", after_colon
                ));
            } else {
                // Если нет двоеточия, выводим ключ в первой колонке с двоеточием, значение во второй
                well_table_html.push_str(&format!(
                    "<tr style='border: none'><td style='text-align: right; padding-right: 5px; border: none'>{}{}</td><td style='text-align: left; padding-left: 5px; border: none'>{}</td></tr>\n",
                    key, ":", value
                ));
            }
        }
        well_table_html.push_str("</table>\n");
    }
    
    // Начинаем таблицу с графиками
    html_before_scale.push_str("<table border='0' cellspacing='0' cellpadding='0' style='border-collapse: collapse; border-spacing: 0; margin: 0; padding: 0; font-family: monospace;'>\n");
    
    // Первая строка с объединённой ячейкой для верхних таблиц
    let colspan = if separate_depth_column { 2 } else { 1 };
    html_before_scale.push_str(&format!(
        "<tr style='border: none; border-width: 0; border-collapse: collapse;'><td colspan='{}' style='padding: 10px 10px 10px 0; border: none; border-width: 0; border-collapse: collapse; vertical-align: top;'><div style='display: flex; gap: 20px; align-items: flex-start;'><div style='vertical-align: top;'>{}</div><div style='vertical-align: top; margin-left: auto; text-align: right;'>{}</div></div></td></tr>\n",
        colspan, curves_table_html, well_table_html
    ));

    // HTML строки таблицы со шкалой
    // Высота шкалы соответствует полной высоте строки
    // Ограничиваем количество кривых для шкалы до max_scales
    let scale_curves_data: Vec<_> = curves_data.iter().take(max_scales).cloned().collect();
    let scale_colors: Vec<_> = colors.iter().take(max_scales).cloned().collect();
    let scale_x_ranges: Vec<_> = x_ranges.iter().take(max_scales).cloned().collect();
    
    let scale_config = PlotConfig {
        width: image_width as u32,
        height: row_height as u32,
        colors: scale_colors,
        x_ranges: scale_x_ranges,
        y_range: (depth_min, depth_max),
        show_scales: true,
        pixels_per_step,
        html_row_steps,
        scale_spacing,
    };

    let scale_png = generate_plot_png(
        &scale_config,
        scale_curves_data,
        &depth_data,
        0,
        html_row_steps.min(depth_data.len()),
    ).map_err(|e| actix_web::error::ErrorInternalServerError(e))?;

    let scale_base64 = base64::engine::general_purpose::STANDARD.encode(&scale_png);
    let html_scale_row = if separate_depth_column {
        format!(
            "<tr style='vertical-align: top; margin: 0; padding: 0;'><td style='padding: 0; margin: 0; border: 1px solid #ccc;'></td><td style='padding: 0; margin: 0; border: 1px solid #ccc; vertical-align: top;'><img src='data:image/png;base64,{}' alt='Scales' width='{}' style='display: block; margin: 0; padding: 0;'></td></tr>\n",
            scale_base64, image_width
        )
    } else {
        format!(
            "<tr style='vertical-align: top; margin: 0; padding: 0;'><td style='padding: 0; margin: 0; border: 1px solid #ccc; vertical-align: top;'><img src='data:image/png;base64,{}' alt='Scales' width='{}' style='display: block; margin: 0; padding: 0;'></td></tr>\n",
            scale_base64, image_width
        )
    };

    // HTML строк с изображением
    // Высота каждого блока: html_row_steps * pixels_per_step + 1
    let block_height = (1+html_row_steps) * pixels_per_step;
    
    let plot_config = Arc::new(PlotConfig {
        width: image_width as u32,
        height: block_height as u32,
        colors: colors.clone(),
        x_ranges: x_ranges.clone(),
        y_range: (depth_min, depth_max),
        show_scales: false,
        pixels_per_step,
        html_row_steps,
        scale_spacing,
    });

    /*
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
    */
    // 1) поток индексов
    let depth_len = depth_data.len();
    let html_plot_rows = stream::iter(0..num_rows)
        // 2) превращаем каждый индекс в future
        .map(move |row_idx| {
            let start_block_value = row_idx * html_row_steps;
            let end_block_value = (start_block_value + html_row_steps).min(depth_data.len());

            // Arc clones
            let plot_config = plot_config.clone();
            let curves_data = curves_data.clone();
            let depth_data = depth_data.clone();
            let num_rows = num_rows;

            async move {
                if start_block_value >= depth_len {
                    return Ok(String::new());
                }

                // Для всех строк кроме последней добавляем еще один шаг из следующей строки
                // (если он существует и не None)
                let is_last_row = row_idx == num_rows - 1;
                let actual_end = if is_last_row {
                    end_block_value
                } else {
                    // Добавляем еще один шаг, если он существует и не None
                    if end_block_value < depth_len {
                        // Проверяем, что следующий шаг не None
                        if depth_data.get(end_block_value).and_then(|&d| d).is_some() {
                            end_block_value + 1
                        } else {
                            end_block_value
                        }
                    } else {
                        end_block_value
                    }
                };
                
                // Высота изображения должна быть равна высоте строки (block_height)
                // независимо от количества шагов в этом блоке
                let image_height = block_height;

                // Создаем PlotConfig с правильной высотой для этого блока
                let block_plot_config = PlotConfig {
                    width: plot_config.width,
                    height: block_height as u32, // Используем фиксированную высоту блока
                    colors: plot_config.colors.clone(),
                    x_ranges: plot_config.x_ranges.clone(),
                    y_range: plot_config.y_range,
                    show_scales: false,
                    pixels_per_step: plot_config.pixels_per_step,
                    html_row_steps: plot_config.html_row_steps,
                    scale_spacing: plot_config.scale_spacing,
                };
                
                generate_html_row(
                    &block_plot_config,
                    curves_data.to_vec(),
                    &depth_data,
                    start_block_value,
                    actual_end,
                    block_height,
                    image_width,
                    image_height,
                    separate_depth_column,
                    depth_min,
                )
                    .await
            }
        })
        // 3) параллельность
        .buffered(2)
        // 4) String → Bytes
        .map(|res| res.map(Bytes::from))
        // 5) приведение типа ошибки
        .map_err(|e| actix_web::error::ErrorInternalServerError(e));

    // HTML конца таблицы и документа
    let html_end = "</table>\n</body></html>\n";

    // Слепляем все части
    //let html = format!("{}{}{}{}", html_before_scale, html_scale_row, html_plot_rows, html_end);
    
    // Оборачиваем в stream через once (фиктивно)
    //let stream = once(ok::<_, actix_web::Error>(Bytes::from(html)));

    let before = once(ok::<_, actix_web::Error>(Bytes::from(html_before_scale + &html_scale_row)));
    let after = once(ok::<_, actix_web::Error>(Bytes::from(html_end)));

    Ok( before.chain(html_plot_rows.chain(after)) )
}
