use std::any::Any;
use std::fs::{self, File};
use std::io::{Read, Write};
use std::panic::{catch_unwind, AssertUnwindSafe};
use std::path::{Path, PathBuf};
use std::sync::{Mutex, OnceLock};

use anyhow::{anyhow, Context, Result};
use calamine::{open_workbook_auto, Data, Reader};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

const OFFICE_EXTS: &[&str] = &["docx", "pptx", "xlsx", "xls", "ods", "odt", "odp"];
const IMAGE_EXTS: &[&str] = &[
    "png", "jpg", "jpeg", "gif", "webp", "bmp", "ico", "tiff", "tif",
];

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SavedImage {
    pub index: u32,
    pub mime_type: String,
    pub page: Option<u32>,
    pub width: u32,
    pub height: u32,
    pub rel_path: String,
    pub abs_path: String,
    pub sha256: String,
}

#[derive(Debug, Clone)]
pub struct DocumentExtraction {
    pub markdown: String,
    pub images: Vec<SavedImage>,
}

#[derive(Debug, Clone)]
struct ExtractOptions {
    min_width: u32,
    min_height: u32,
    max_images: usize,
}

impl Default for ExtractOptions {
    fn default() -> Self {
        Self {
            min_width: 100,
            min_height: 100,
            max_images: 500,
        }
    }
}

static PDFIUM: OnceLock<Result<pdfium_render::prelude::Pdfium, String>> = OnceLock::new();
static PDFIUM_LOCK: Mutex<()> = Mutex::new(());
static RESOURCE_DIR_HINT: OnceLock<PathBuf> = OnceLock::new();

pub fn set_resource_dir_hint(dir: PathBuf) {
    let _ = RESOURCE_DIR_HINT.set(dir);
}

pub fn extract_source_content(path: &Path, collection_root: &Path) -> Result<DocumentExtraction> {
    run_guarded("knowledge_extract_source", || {
        extract_source_content_inner(path, collection_root).map_err(|error| error.to_string())
    })
    .map_err(|error| anyhow!(error))
}

fn extract_source_content_inner(path: &Path, collection_root: &Path) -> Result<DocumentExtraction> {
    let ext = path
        .extension()
        .and_then(|ext| ext.to_str())
        .unwrap_or("")
        .to_ascii_lowercase();
    let source_slug = source_slug(path);
    let media_dir = collection_root
        .join("wiki")
        .join("media")
        .join(&source_slug);
    let media_prefix = format!("media/{source_slug}");
    let options = ExtractOptions::default();

    match ext.as_str() {
        "md" | "markdown" | "txt" | "csv" | "json" | "toml" | "yaml" | "yml" | "rs" | "ts"
        | "tsx" | "js" | "jsx" | "py" | "go" | "java" | "c" | "cpp" | "h" | "hpp" => {
            let raw = fs::read_to_string(path)
                .with_context(|| format!("读取文本源文件失败：{}", path.display()))?;
            Ok(DocumentExtraction {
                markdown: raw,
                images: Vec::new(),
            })
        }
        "pdf" => extract_pdf_markdown(path, &media_dir, &media_prefix, &options),
        e if OFFICE_EXTS.contains(&e) => {
            let mut markdown = extract_office_text(path, e)?;
            let images = if matches!(e, "docx" | "pptx" | "xlsx" | "xls" | "ods") {
                extract_and_save_office_images(
                    path,
                    &media_dir,
                    &collection_root.join("wiki"),
                    &options,
                )?
            } else {
                Vec::new()
            };
            append_image_refs(&mut markdown, &images);
            Ok(DocumentExtraction { markdown, images })
        }
        e if IMAGE_EXTS.contains(&e) => {
            extract_standalone_image(path, &media_dir, &collection_root.join("wiki"), e)
        }
        _ => {
            let file = fs::File::open(path)?;
            let mut buf = Vec::new();
            file.take(2 * 1024 * 1024).read_to_end(&mut buf)?;
            let markdown = String::from_utf8(buf)
                .map_err(|_| anyhow!("源文件不是 UTF-8 文本：{}", path.display()))?;
            Ok(DocumentExtraction {
                markdown,
                images: Vec::new(),
            })
        }
    }
}

fn extract_pdf_markdown(
    path: &Path,
    media_dir: &Path,
    media_prefix: &str,
    options: &ExtractOptions,
) -> Result<DocumentExtraction> {
    use pdfium_render::prelude::*;

    let _guard = lock_pdfium();
    let pdfium = pdfium()?;
    let path_str = path.to_string_lossy();
    let doc = pdfium
        .load_pdf_from_file(path, None)
        .map_err(|error| match error {
            PdfiumError::PdfiumLibraryInternalError(PdfiumInternalError::PasswordError) => {
                anyhow!("PDF 受密码保护，无法读取：{}", path.display())
            }
            _ => anyhow!("打开 PDF 失败：{}：{}", path.display(), error),
        })?;

    let mut markdown = String::new();
    let mut images = Vec::new();
    let mut image_index = 0u32;

    'pages: for (page_idx, page) in doc.pages().iter().enumerate() {
        let page_num = (page_idx + 1) as u32;
        if !markdown.is_empty() {
            markdown.push_str("\n\n");
        }
        markdown.push_str(&format!("## Page {page_num}\n\n"));
        let page_text = page.text().map_err(|error| {
            anyhow!("PDF 第 {page_num} 页文本提取失败：{}：{}", path_str, error)
        })?;
        markdown.push_str(&page_text.all());
        markdown.push('\n');

        let mut page_image_lines = Vec::new();
        for object in page.objects().iter() {
            let Some(image) = object.as_image_object() else {
                continue;
            };
            let dyn_img = match image.get_raw_image() {
                Ok(image) => image,
                Err(error) => {
                    eprintln!("[knowledge_pdf] page {page_num} image read failed: {error}");
                    continue;
                }
            };
            let width = dyn_img.width();
            let height = dyn_img.height();
            if width < options.min_width || height < options.min_height {
                continue;
            }

            let mut png_bytes = Vec::new();
            if let Err(error) = dyn_img.write_to(
                &mut std::io::Cursor::new(&mut png_bytes),
                image::ImageFormat::Png,
            ) {
                eprintln!("[knowledge_pdf] page {page_num} PNG encode failed: {error}");
                continue;
            }

            image_index += 1;
            let file_name = format!("img-{image_index}.png");
            let (_, abs_path) =
                save_one_image(&png_bytes, media_dir, &media_dir.join("../.."), &file_name)?;
            let rel_path = format!("{media_prefix}/{file_name}");
            let sha256 = sha256_hex(&png_bytes);

            images.push(SavedImage {
                index: image_index,
                mime_type: "image/png".to_string(),
                page: Some(page_num),
                width,
                height,
                rel_path: rel_path.clone(),
                abs_path,
                sha256,
            });
            page_image_lines.push(format!("![PDF image {image_index}]({rel_path})"));

            if images.len() >= options.max_images {
                break 'pages;
            }
        }

        if !page_image_lines.is_empty() {
            markdown.push('\n');
            markdown.push_str(&page_image_lines.join("\n"));
            markdown.push('\n');
        }
    }

    Ok(DocumentExtraction { markdown, images })
}

fn extract_office_text(path: &Path, ext: &str) -> Result<String> {
    if matches!(ext, "xlsx" | "xls" | "ods") {
        return extract_spreadsheet(path);
    }
    if ext == "docx" {
        return extract_docx_with_library(path);
    }

    let file = fs::File::open(path)
        .with_context(|| format!("打开 Office 文件失败：{}", path.display()))?;
    let mut archive = zip::ZipArchive::new(file)
        .with_context(|| format!("读取 Office ZIP 失败：{}", path.display()))?;

    match ext {
        "pptx" => extract_pptx_markdown(&mut archive),
        "odt" | "odp" => extract_odf_text(&mut archive),
        _ => Err(anyhow!("不支持的 Office 格式：{ext}")),
    }
}

fn extract_docx_with_library(path: &Path) -> Result<String> {
    let bytes = fs::read(path).with_context(|| format!("读取 DOCX 失败：{}", path.display()))?;
    let docx = docx_rs::read_docx(&bytes)
        .map_err(|error| anyhow!("解析 DOCX 失败：{}：{:?}", path.display(), error))?;

    let mut result = String::new();
    for child in docx.document.children {
        match child {
            docx_rs::DocumentChild::Paragraph(para) => {
                let mut para_text = String::new();
                let mut is_heading = false;
                let mut heading_level: u8 = 1;
                if let Some(style) = &para.property.style {
                    let style_val = &style.val;
                    if style_val.contains("Heading") || style_val.contains("heading") {
                        is_heading = true;
                        if let Some(level) = style_val.chars().find(|ch| ch.is_ascii_digit()) {
                            heading_level = level.to_digit(10).unwrap_or(1) as u8;
                        }
                    }
                }
                let is_list = para.property.numbering_property.is_some();

                for child in &para.children {
                    let docx_rs::ParagraphChild::Run(run) = child else {
                        continue;
                    };
                    let is_bold = run.run_property.bold.is_some();
                    let is_italic = run.run_property.italic.is_some();
                    for run_child in &run.children {
                        if let docx_rs::RunChild::Text(text) = run_child {
                            let t = &text.text;
                            if is_bold && is_italic {
                                para_text.push_str(&format!("***{t}***"));
                            } else if is_bold {
                                para_text.push_str(&format!("**{t}**"));
                            } else if is_italic {
                                para_text.push_str(&format!("*{t}*"));
                            } else {
                                para_text.push_str(t);
                            }
                        }
                    }
                }

                let text = para_text.trim();
                if text.is_empty() {
                    continue;
                }
                if is_heading {
                    result.push_str(&format!(
                        "{} {text}\n\n",
                        "#".repeat(heading_level as usize)
                    ));
                } else if is_list {
                    result.push_str(&format!("- {text}\n"));
                } else {
                    result.push_str(text);
                    result.push_str("\n\n");
                }
            }
            docx_rs::DocumentChild::Table(table) => {
                let rows = docx_table_rows(&table);
                append_markdown_table(&mut result, rows);
            }
            _ => {}
        }
    }

    if result.trim().is_empty() {
        let file = fs::File::open(path)?;
        let mut archive = zip::ZipArchive::new(file)?;
        extract_docx_markdown(&mut archive)
    } else {
        Ok(result)
    }
}

fn docx_table_rows(table: &docx_rs::Table) -> Vec<Vec<String>> {
    let mut rows = Vec::new();
    for row in &table.rows {
        let docx_rs::TableChild::TableRow(tr) = row;
        let mut cells = Vec::new();
        for cell in &tr.cells {
            let docx_rs::TableRowChild::TableCell(tc) = cell;
            let mut cell_text = String::new();
            for child in &tc.children {
                let docx_rs::TableCellContent::Paragraph(para) = child else {
                    continue;
                };
                for pchild in &para.children {
                    let docx_rs::ParagraphChild::Run(run) = pchild else {
                        continue;
                    };
                    for rc in &run.children {
                        if let docx_rs::RunChild::Text(t) = rc {
                            cell_text.push_str(&t.text);
                        }
                    }
                }
            }
            cells.push(cell_text.trim().replace('|', "\\|"));
        }
        rows.push(cells);
    }
    rows
}

fn read_zip_file(archive: &mut zip::ZipArchive<fs::File>, name: &str) -> Option<String> {
    let mut file = archive.by_name(name).ok()?;
    let mut content = String::new();
    file.read_to_string(&mut content).ok()?;
    Some(content)
}

fn decode_xml_entities(text: &str) -> String {
    text.replace("&amp;", "&")
        .replace("&lt;", "<")
        .replace("&gt;", ">")
        .replace("&quot;", "\"")
        .replace("&apos;", "'")
        .replace("&#10;", "\n")
        .replace("&#13;", "")
}

fn extract_docx_markdown(archive: &mut zip::ZipArchive<fs::File>) -> Result<String> {
    let xml = read_zip_file(archive, "word/document.xml")
        .ok_or_else(|| anyhow!("DOCX 缺少 word/document.xml"))?;
    let mut result = String::new();
    let mut text = String::new();
    let mut in_text = false;
    for part in xml.split('<') {
        if let Some(rest) = part.strip_prefix("w:t") {
            in_text = true;
            if let Some(pos) = rest.find('>') {
                text.push_str(&decode_xml_entities(&rest[pos + 1..]));
            }
        } else if part.starts_with("/w:t") {
            in_text = false;
        } else if in_text {
            text.push_str(&decode_xml_entities(
                part.split('>').nth(1).unwrap_or_default(),
            ));
        }
        if part.starts_with("/w:p") && !text.trim().is_empty() {
            result.push_str(text.trim());
            result.push_str("\n\n");
            text.clear();
        }
    }
    if result.trim().is_empty() {
        Ok("[Could not extract structured text from DOCX]".to_string())
    } else {
        Ok(result)
    }
}

fn extract_pptx_markdown(archive: &mut zip::ZipArchive<fs::File>) -> Result<String> {
    let mut slide_names: Vec<String> = (0..archive.len())
        .filter_map(|i| archive.by_index(i).ok().map(|file| file.name().to_string()))
        .filter(|name| name.starts_with("ppt/slides/slide") && name.ends_with(".xml"))
        .collect();
    slide_names.sort_by_key(|name| {
        name.trim_start_matches("ppt/slides/slide")
            .trim_end_matches(".xml")
            .parse::<u32>()
            .unwrap_or(0)
    });

    let mut result = String::new();
    for (idx, slide_name) in slide_names.iter().enumerate() {
        let Some(xml) = read_zip_file(archive, slide_name) else {
            continue;
        };
        result.push_str(&format!("## Slide {}\n\n", idx + 1));
        let mut paragraphs = Vec::new();
        for para_part in xml.split("<a:p") {
            let mut para_text = String::new();
            for t_part in para_part.split("<a:t") {
                if let Some(close_pos) = t_part.find("</a:t>") {
                    if let Some(gt_pos) = t_part.find('>') {
                        if gt_pos < close_pos {
                            para_text
                                .push_str(&decode_xml_entities(&t_part[gt_pos + 1..close_pos]));
                        }
                    }
                }
            }
            let trimmed = para_text.trim().to_string();
            if !trimmed.is_empty() {
                paragraphs.push(trimmed);
            }
        }
        if let Some(title) = paragraphs.first() {
            result.push_str(&format!("**{title}**\n\n"));
            for para in paragraphs.iter().skip(1) {
                result.push_str(&format!("- {para}\n"));
            }
        }
        result.push('\n');
    }

    if result.trim().is_empty() {
        Ok("[Could not extract text from PPTX]".to_string())
    } else {
        Ok(result)
    }
}

fn extract_spreadsheet(path: &Path) -> Result<String> {
    let mut workbook = open_workbook_auto(path)
        .with_context(|| format!("打开电子表格失败：{}", path.display()))?;
    let sheet_names = workbook.sheet_names().to_vec();
    let mut result = String::new();

    for sheet_name in &sheet_names {
        if let Ok(range) = workbook.worksheet_range(sheet_name) {
            if range.is_empty() {
                continue;
            }
            if sheet_names.len() > 1 {
                result.push_str(&format!("## {sheet_name}\n\n"));
            }
            let rows = range
                .rows()
                .map(|row| {
                    row.iter()
                        .map(|cell| match cell {
                            Data::Empty => String::new(),
                            Data::String(s) => s.clone(),
                            Data::Float(f) => {
                                if *f == (*f as i64) as f64 {
                                    (*f as i64).to_string()
                                } else {
                                    format!("{f:.2}")
                                }
                            }
                            Data::Int(i) => i.to_string(),
                            Data::Bool(b) => b.to_string(),
                            Data::DateTime(dt) => dt.to_string(),
                            Data::DateTimeIso(s) => s.clone(),
                            Data::DurationIso(s) => s.clone(),
                            Data::Error(e) => format!("ERR:{e:?}"),
                        })
                        .collect::<Vec<_>>()
                })
                .collect::<Vec<_>>();
            append_markdown_table(&mut result, rows);
        }
    }

    if result.trim().is_empty() {
        Ok("[Could not extract data from spreadsheet]".to_string())
    } else {
        Ok(result)
    }
}

fn extract_odf_text(archive: &mut zip::ZipArchive<fs::File>) -> Result<String> {
    let xml =
        read_zip_file(archive, "content.xml").ok_or_else(|| anyhow!("ODF 缺少 content.xml"))?;
    let mut result = String::new();
    let mut in_tag = false;
    for ch in xml.chars() {
        match ch {
            '<' => in_tag = true,
            '>' => {
                in_tag = false;
                result.push(' ');
            }
            _ if !in_tag => result.push(ch),
            _ => {}
        }
    }
    let cleaned = decode_xml_entities(&result);
    let lines = cleaned
        .lines()
        .map(str::trim)
        .filter(|line| !line.is_empty())
        .collect::<Vec<_>>();
    if lines.is_empty() {
        Ok("[Could not extract text from this file]".to_string())
    } else {
        Ok(lines.join("\n\n"))
    }
}

fn append_markdown_table(result: &mut String, rows: Vec<Vec<String>>) {
    if rows.is_empty() {
        return;
    }
    let max_cols = rows.iter().map(Vec::len).max().unwrap_or(0);
    if max_cols == 0 {
        return;
    }
    for (i, row) in rows.iter().enumerate() {
        let mut padded = row.clone();
        padded.resize(max_cols, String::new());
        let escaped = padded
            .iter()
            .map(|cell| cell.replace('|', "\\|"))
            .collect::<Vec<_>>();
        result.push_str("| ");
        result.push_str(&escaped.join(" | "));
        result.push_str(" |\n");
        if i == 0 {
            result.push('|');
            for _ in 0..max_cols {
                result.push_str(" --- |");
            }
            result.push('\n');
        }
    }
    result.push('\n');
}

fn extract_and_save_office_images(
    path: &Path,
    dest_dir: &Path,
    rel_to: &Path,
    options: &ExtractOptions,
) -> Result<Vec<SavedImage>> {
    let file =
        File::open(path).with_context(|| format!("打开 Office 图片源失败：{}", path.display()))?;
    let mut archive = zip::ZipArchive::new(file)
        .with_context(|| format!("读取 Office 图片 ZIP 失败：{}", path.display()))?;
    let is_pptx = archive
        .file_names()
        .any(|name| name == "ppt/presentation.xml" || name.starts_with("ppt/slides/slide"));
    let media_to_slide = if is_pptx {
        build_pptx_media_slide_map(&mut archive)
    } else {
        std::collections::HashMap::new()
    };
    let media_indices = (0..archive.len())
        .filter(|i| {
            archive
                .by_index_raw(*i)
                .ok()
                .map(|f| is_media_path(f.name()))
                .unwrap_or(false)
        })
        .collect::<Vec<_>>();

    let mut out = Vec::new();
    let mut idx = 0u32;
    for archive_idx in media_indices {
        let mut entry = match archive.by_index(archive_idx) {
            Ok(entry) => entry,
            Err(error) => {
                eprintln!("[knowledge_office_images] zip entry read failed: {error}");
                continue;
            }
        };
        let entry_name = entry.name().to_string();
        let Some(mime_type) = guess_mime_from_name(&entry_name) else {
            continue;
        };
        let mut bytes = Vec::with_capacity(entry.size() as usize);
        if let Err(error) = entry.read_to_end(&mut bytes) {
            eprintln!("[knowledge_office_images] read '{entry_name}' failed: {error}");
            continue;
        }
        let (width, height) = match image::load_from_memory(&bytes) {
            Ok(image) => (image.width(), image.height()),
            Err(error) => {
                eprintln!("[knowledge_office_images] decode '{entry_name}' failed: {error}");
                continue;
            }
        };
        if width < options.min_width || height < options.min_height {
            continue;
        }

        idx += 1;
        let file_name = format!("img-{idx}.{}", ext_for_mime(&mime_type));
        let (rel_path, abs_path) = save_one_image(&bytes, dest_dir, rel_to, &file_name)?;
        out.push(SavedImage {
            index: idx,
            mime_type,
            page: media_to_slide.get(&entry_name).copied().flatten(),
            width,
            height,
            rel_path,
            abs_path,
            sha256: sha256_hex(&bytes),
        });
        if out.len() >= options.max_images {
            break;
        }
    }
    Ok(out)
}

fn extract_standalone_image(
    path: &Path,
    dest_dir: &Path,
    rel_to: &Path,
    ext: &str,
) -> Result<DocumentExtraction> {
    let mime_type = guess_mime_from_name(
        path.file_name()
            .and_then(|name| name.to_str())
            .unwrap_or(""),
    )
    .unwrap_or_else(|| format!("image/{ext}"));
    let bytes = fs::read(path).with_context(|| format!("读取图片失败：{}", path.display()))?;
    let (width, height) = image::load_from_memory(&bytes)
        .map(|image| (image.width(), image.height()))
        .unwrap_or((0, 0));
    let file_name = format!("source.{}", ext_for_mime(&mime_type));
    let (rel_path, abs_path) = save_one_image(&bytes, dest_dir, rel_to, &file_name)?;
    let image = SavedImage {
        index: 1,
        mime_type,
        page: None,
        width,
        height,
        rel_path: rel_path.clone(),
        abs_path,
        sha256: sha256_hex(&bytes),
    };
    Ok(DocumentExtraction {
        markdown: format!(
            "![{}]({})\n",
            path.file_name().unwrap_or_default().to_string_lossy(),
            rel_path
        ),
        images: vec![image],
    })
}

fn append_image_refs(markdown: &mut String, images: &[SavedImage]) {
    if images.is_empty() {
        return;
    }
    if !markdown.ends_with('\n') {
        markdown.push('\n');
    }
    markdown.push_str("\n## Extracted Images\n\n");
    for image in images {
        markdown.push_str(&format!("![Image {}]({})\n\n", image.index, image.rel_path));
    }
}

fn save_one_image(
    bytes: &[u8],
    dest_dir: &Path,
    rel_to: &Path,
    file_name: &str,
) -> Result<(String, String)> {
    fs::create_dir_all(dest_dir)
        .with_context(|| format!("创建图片目录失败：{}", dest_dir.display()))?;
    let abs = dest_dir.join(file_name);
    let mut file =
        fs::File::create(&abs).with_context(|| format!("创建图片失败：{}", abs.display()))?;
    file.write_all(bytes)
        .with_context(|| format!("写入图片失败：{}", abs.display()))?;
    let rel = abs
        .strip_prefix(rel_to)
        .map(|path| path.to_string_lossy().replace('\\', "/"))
        .unwrap_or_else(|_| file_name.to_string());
    Ok((rel, abs.to_string_lossy().replace('\\', "/")))
}

fn is_media_path(name: &str) -> bool {
    let lower = name.to_ascii_lowercase();
    lower.starts_with("ppt/media/")
        || lower.starts_with("word/media/")
        || lower.starts_with("xl/media/")
}

fn guess_mime_from_name(name: &str) -> Option<String> {
    let ext = Path::new(name)
        .extension()
        .and_then(|ext| ext.to_str())?
        .to_ascii_lowercase();
    match ext.as_str() {
        "png" => Some("image/png".to_string()),
        "jpg" | "jpeg" => Some("image/jpeg".to_string()),
        "gif" => Some("image/gif".to_string()),
        "webp" => Some("image/webp".to_string()),
        "bmp" => Some("image/bmp".to_string()),
        "ico" => Some("image/x-icon".to_string()),
        "tif" | "tiff" => Some("image/tiff".to_string()),
        _ => None,
    }
}

fn ext_for_mime(mime: &str) -> &'static str {
    match mime {
        "image/png" => "png",
        "image/jpeg" => "jpg",
        "image/gif" => "gif",
        "image/webp" => "webp",
        "image/bmp" => "bmp",
        "image/x-icon" => "ico",
        "image/tiff" => "tiff",
        _ => "bin",
    }
}

fn build_pptx_media_slide_map(
    archive: &mut zip::ZipArchive<File>,
) -> std::collections::HashMap<String, Option<u32>> {
    use std::collections::HashMap;
    let mut out = HashMap::new();
    let rels_paths = archive
        .file_names()
        .filter(|name| name.starts_with("ppt/slides/_rels/slide") && name.ends_with(".xml.rels"))
        .map(String::from)
        .collect::<Vec<_>>();
    for rels_path in rels_paths {
        let slide_num = rels_path
            .strip_prefix("ppt/slides/_rels/slide")
            .and_then(|s| s.strip_suffix(".xml.rels"))
            .and_then(|s| s.parse().ok());
        let mut entry = match archive.by_name(&rels_path) {
            Ok(entry) => entry,
            Err(_) => continue,
        };
        let mut xml = String::new();
        if entry.read_to_string(&mut xml).is_err() {
            continue;
        }
        let mut search_from = 0;
        while let Some(pos) = xml[search_from..].find("Target=\"") {
            let start = search_from + pos + "Target=\"".len();
            let Some(end_rel) = xml[start..].find('"') else {
                break;
            };
            let end = start + end_rel;
            let target = &xml[start..end];
            search_from = end + 1;
            if let Some(stripped) = target.strip_prefix("../") {
                let canonical = format!("ppt/{stripped}");
                if is_media_path(&canonical) {
                    out.insert(canonical, slide_num);
                }
            }
        }
    }
    out
}

fn source_slug(path: &Path) -> String {
    let stem = path
        .file_stem()
        .and_then(|name| name.to_str())
        .unwrap_or("source");
    let mut out = String::new();
    let mut last_dash = false;
    for ch in stem.to_lowercase().chars() {
        if ch.is_ascii_alphanumeric() {
            out.push(ch);
            last_dash = false;
        } else if !last_dash {
            out.push('-');
            last_dash = true;
        }
    }
    let trimmed = out.trim_matches('-');
    if trimmed.is_empty() {
        "source".to_string()
    } else {
        trimmed.to_string()
    }
}

fn sha256_hex(bytes: &[u8]) -> String {
    let mut hasher = Sha256::new();
    hasher.update(bytes);
    let digest = hasher.finalize();
    let mut out = String::with_capacity(digest.len() * 2);
    for byte in digest {
        out.push_str(&format!("{byte:02x}"));
    }
    out
}

fn lock_pdfium() -> std::sync::MutexGuard<'static, ()> {
    PDFIUM_LOCK
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}

fn pdfium() -> Result<&'static pdfium_render::prelude::Pdfium> {
    PDFIUM
        .get_or_init(|| {
            use pdfium_render::prelude::*;
            let candidates = pdfium_candidate_paths();
            for path in &candidates {
                if let Ok(bindings) = Pdfium::bind_to_library(path) {
                    eprintln!("[knowledge_pdfium] loaded dynamic library from {path}");
                    return Ok(Pdfium::new(bindings));
                }
            }
            Pdfium::bind_to_system_library().map(Pdfium::new).map_err(|error| {
                format!(
                    "Failed to locate Pdfium library. Tried: {} — and system search path. Last error: {error}",
                    if candidates.is_empty() { "(no candidates)".to_string() } else { candidates.join(", ") }
                )
            })
        })
        .as_ref()
        .map_err(|error| anyhow!(error.clone()))
}

fn pdfium_candidate_paths() -> Vec<String> {
    let mut out = Vec::new();
    if let Ok(path) = std::env::var("PDFIUM_DYNAMIC_LIB_PATH") {
        out.push(path);
    }
    if let Some(resource_dir) = RESOURCE_DIR_HINT.get() {
        push_pdfium_candidates(&mut out, resource_dir);
    }
    if let Ok(current_dir) = std::env::current_dir() {
        push_pdfium_candidates(&mut out, &current_dir);
        push_pdfium_candidates(&mut out, &current_dir.join("src-tauri"));
    }
    if let Ok(exe) = std::env::current_exe() {
        if let Some(exe_dir) = exe.parent() {
            push_pdfium_candidates(&mut out, exe_dir);
            push_pdfium_candidates(&mut out, &exe_dir.join("../Resources"));
            push_pdfium_candidates(&mut out, &exe_dir.join("../Frameworks"));
            push_pdfium_candidates(&mut out, &exe_dir.join("resources"));
        }
    }
    out
}

fn push_pdfium_candidates(out: &mut Vec<String>, base: &Path) {
    #[cfg(target_os = "macos")]
    {
        out.push(
            base.join("pdfium")
                .join("libpdfium.dylib")
                .to_string_lossy()
                .into_owned(),
        );
        out.push(base.join("libpdfium.dylib").to_string_lossy().into_owned());
    }
    #[cfg(target_os = "windows")]
    {
        out.push(
            base.join("pdfium")
                .join("pdfium.dll")
                .to_string_lossy()
                .into_owned(),
        );
        out.push(base.join("pdfium.dll").to_string_lossy().into_owned());
        out.push(base.join("libpdfium.dll").to_string_lossy().into_owned());
    }
    #[cfg(target_os = "linux")]
    {
        out.push(
            base.join("pdfium")
                .join("libpdfium.so")
                .to_string_lossy()
                .into_owned(),
        );
        out.push(base.join("libpdfium.so").to_string_lossy().into_owned());
    }
}

fn run_guarded<T, F>(label: &str, f: F) -> Result<T, String>
where
    F: FnOnce() -> Result<T, String>,
{
    match catch_unwind(AssertUnwindSafe(f)) {
        Ok(result) => result,
        Err(payload) => Err(report_panic(label, payload)),
    }
}

fn report_panic(label: &str, payload: Box<dyn Any + Send>) -> String {
    let msg = if let Some(s) = payload.downcast_ref::<String>() {
        s.clone()
    } else if let Some(s) = payload.downcast_ref::<&str>() {
        (*s).to_string()
    } else {
        "(non-string panic payload)".to_string()
    };
    eprintln!("[knowledge_panic_guard] '{label}' panicked: {msg}");
    format!("Internal error in {label}: {msg}")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn source_slug_normalizes_names() {
        assert_eq!(
            source_slug(Path::new("/tmp/My Report 2026.pdf")),
            "my-report-2026"
        );
    }

    #[test]
    fn media_path_recognizes_office_locations() {
        assert!(is_media_path("ppt/media/image1.png"));
        assert!(is_media_path("word/media/image2.jpeg"));
        assert!(is_media_path("xl/media/image3.webp"));
        assert!(!is_media_path("docProps/thumbnail.jpeg"));
    }

    #[test]
    fn panic_guard_converts_panic_to_error() {
        let err = run_guarded::<(), _>("test", || panic!("boom")).unwrap_err();
        assert!(err.contains("boom"));
    }
}
