use anyhow::Result;
use std::collections::HashMap;
use std::str::FromStr;

#[derive(Debug, Clone)]
pub struct LasFile {
    pub version: String,
    pub well_info: HashMap<String, String>,
    pub curves: Vec<CurveInfo>,
    pub parameters: HashMap<String, String>,
    pub data: Vec<DataRow>,
    pub null_value: f64,
}

#[derive(Debug, Clone)]
pub struct CurveInfo {
    pub mnemonic: String,
    pub unit: String,
    pub description: String,
    pub api_codes: Option<String>,
}

#[derive(Debug, Clone)]
pub struct DataRow {
    pub values: Vec<f64>,
}

impl LasFile {
    pub fn parse(content: &str) -> Result<Self> {
        let lines: Vec<&str> = content.lines().collect();
        let mut version = String::new();
        let mut well_info = HashMap::new();
        let mut curves = Vec::new();
        let mut parameters = HashMap::new();
        let mut data = Vec::new();
        let mut null_value = -999.25;

        let mut i = 0;
        let mut in_section = None;

        while i < lines.len() {
            let line = lines[i].trim();
            
            if line.is_empty() || line.starts_with('#') {
                i += 1;
                continue;
            }

            // Определяем секцию
            if line.starts_with('~') {
                let section = line.to_uppercase();
                if section.contains("VERSION") {
                    in_section = Some("version");
                } else if section.contains("WELL") {
                    in_section = Some("well");
                } else if section.contains("CURVE") {
                    in_section = Some("curve");
                } else if section.contains("PARAMETER") {
                    in_section = Some("parameter");
                } else if section.contains("ASCII") || section.contains("~A ") {
                    in_section = Some("data");
                } else {
                    in_section = None;
                }
                i += 1;
                continue;
            }

            match in_section {
                Some("version") => {
                    if let Some((key, value)) = Self::parse_key_value(line) {
                        if key == "VERS." {
                            version = value.trim().to_string();
                        }
                    }
                }
                Some("well") => {
                    if let Some((key, value)) = Self::parse_key_value(line) {
                        let key_upper = key.to_uppercase();
                        if key_upper == "NULL" {
                            if let Ok(val) = f64::from_str(value.trim()) {
                                null_value = val;
                            }
                        }
                        well_info.insert(key.to_string(), value.trim().to_string());
                    }
                }
                Some("curve") => {
                    if line.starts_with('#') {
                        i += 1;
                        continue;
                    }
                    if let Some(curve) = Self::parse_curve_line(line) {
                        curves.push(curve);
                    }
                }
                Some("parameter") => {
                    if line.starts_with('#') {
                        i += 1;
                        continue;
                    }
                    if let Some((key, value)) = Self::parse_key_value(line) {
                        parameters.insert(key.to_string(), value.trim().to_string());
                    }
                }
                Some("data") => {
                    if line.starts_with('#') {
                        i += 1;
                        continue;
                    }
                    if let Some(row) = Self::parse_data_line(line) {
                        data.push(row);
                    }
                }
                _ => {}
            }
            i += 1;
        }

        Ok(LasFile {
            version,
            well_info,
            curves,
            parameters,
            data,
            null_value,
        })
    }

    fn parse_key_value(line: &str) -> Option<(String, String)> {
        let parts: Vec<&str> = line.splitn(3, ':').collect();
        if parts.len() >= 2 {
            let key_part = parts[0].trim();
            let value_part = parts[1].trim();
            if let Some(space_idx) = key_part.find(char::is_whitespace) {
                let key = key_part[..space_idx].to_string();
                return Some((key, value_part.to_string()));
            }
        }
        None
    }

    fn parse_curve_line(line: &str) -> Option<CurveInfo> {
        let parts: Vec<&str> = line.split(':').collect();
        if parts.is_empty() {
            return None;
        }

        let first_part = parts[0].trim();
        let description = parts.get(1).map(|s| s.trim().to_string()).unwrap_or_default();

        // Парсим формат: " MNEM    .UNIT         API CODE" или "MNEM.UNIT API_CODE"
        // Ищем точку, которая разделяет мнемонику и единицу измерения
        let dot_idx = first_part.find('.')?;
        
        // Мнемоника - все до точки (с удалением пробелов)
        let mnemonic = first_part[..dot_idx].trim().to_string();
        
        // После точки ищем единицу измерения (до следующего пробела или конца)
        let after_dot = &first_part[dot_idx + 1..];
        let unit_end = after_dot.find(char::is_whitespace).unwrap_or(after_dot.len());
        let unit = after_dot[..unit_end].trim().to_string();
        
        // API коды - все что после единицы измерения (если есть)
        let rest = after_dot[unit_end..].trim();
        let api_codes = if rest.is_empty() {
            None
        } else {
            Some(rest.to_string())
        };

        Some(CurveInfo {
            mnemonic,
            unit,
            description,
            api_codes,
        })
    }

    fn parse_data_line(line: &str) -> Option<DataRow> {
        let values: Vec<f64> = line
            .split_whitespace()
            .filter_map(|s| f64::from_str(s).ok())
            .collect();
        
        if values.is_empty() {
            None
        } else {
            Some(DataRow { values })
        }
    }

    pub fn get_curve_index(&self, mnemonic: &str) -> Option<usize> {
        self.curves
            .iter()
            .position(|c| c.mnemonic.to_uppercase() == mnemonic.to_uppercase())
    }

    pub fn get_main_parameter_index(&self, main_param: &str) -> Option<usize> {
        self.get_curve_index(main_param)
    }

    pub fn get_curve_data(&self, curve_idx: usize) -> Vec<Option<f64>> {
        self.data
            .iter()
            .map(|row| {
                if curve_idx < row.values.len() {
                    let val = row.values[curve_idx];
                    if val == self.null_value || val.is_nan() {
                        None
                    } else {
                        Some(val)
                    }
                } else {
                    None
                }
            })
            .collect()
    }

    pub fn get_curve_stats(&self, curve_idx: usize) -> Option<(f64, f64)> {
        let data = self.get_curve_data(curve_idx);
        let valid_data: Vec<f64> = data.iter().filter_map(|&x| x).collect();
        
        if valid_data.is_empty() {
            return None;
        }

        let min = valid_data.iter().fold(f64::INFINITY, |a, &b| a.min(b));
        let max = valid_data.iter().fold(f64::NEG_INFINITY, |a, &b| a.max(b));
        
        Some((min, max))
    }
}

