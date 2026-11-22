use crate::data::ScoreFile;
use pdfium_render::prelude::{PdfBitmapFormat, PdfColor, PdfDocument, PdfRenderConfig, Pdfium};
use std::error::Error;
use std::fmt::Write;
use std::fs::File;
use std::path::{Path, PathBuf};
use std::time::Instant;

type PdfResult<T> = Result<T, Box<dyn Error>>;

pub fn open_engine() -> PdfResult<Pdfium> {
    let path = std::env::current_exe()?.with_file_name("pdfium.dll");
    let bindings = Pdfium::bind_to_library(&path).map_err(|error| {
        trf!(
            "Impossibile caricare PDFium da {}. {error}",
            "Couldn’t load PDFium from {}. {error}",
            path.display()
        )
    })?;
    Ok(Pdfium::new(bindings))
}

pub struct Frame {
    pub width: i32,
    pub height: i32,
    pub pixels: Vec<u8>,
}

pub fn preview_dimensions(width: u32, height: u32) -> Option<(u32, u32)> {
    const LONG_EDGE: u32 = 640;
    let edge = width.max(height);
    (edge >= 768).then(|| {
        (
            (width * LONG_EDGE / edge).max(1),
            (height * LONG_EDGE / edge).max(1),
        )
    })
}

pub struct Score<'a> {
    pub source: ScoreFile,
    file: File,
    document: PdfDocument<'a>,
    pub first_page: u32,
    pub count: u32,
}

impl<'a> Score<'a> {
    pub fn open(engine: &'a Pdfium, path: &Path, first_page: u32) -> PdfResult<Self> {
        let (source, file) = ScoreFile::open(path)?;
        let document = engine.load_pdf_from_reader(file.try_clone()?, None)?;
        let count = document.pages().len().try_into()?;
        source.verify_revision(&file)?;
        Ok(Self {
            source,
            file,
            document,
            first_page,
            count,
        })
    }

    pub fn render(&self, global_page: u32, width: u32, height: u32) -> PdfResult<Frame> {
        let page = self
            .document
            .pages()
            .get((global_page - self.first_page).try_into()?)?;
        let page_width = page.width().value;
        let page_height = page.height().value;
        let scale = (width as f32 / page_width).min(height as f32 / page_height);
        let width = ((page_width * scale).round() as u32).clamp(1, width) as i32;
        let height = ((page_height * scale).round() as u32).clamp(1, height) as i32;
        let config = PdfRenderConfig::new()
            .set_fixed_size(width, height)
            .set_format(PdfBitmapFormat::BGRA)
            .set_reverse_byte_order(false)
            .set_clear_color(PdfColor::WHITE);
        let bitmap = page.render_with_config(&config)?;
        let width = bitmap.width();
        let height = bitmap.height();
        let pixels = bitmap.as_raw_bytes();
        if width <= 0 || height <= 0 || pixels.len() != width as usize * height as usize * 4 {
            return Err(tr!(
                "Il motore PDF ha restituito un'immagine non valida.",
                "The PDF engine returned an invalid image."
            )
            .into());
        }
        self.source.verify_revision(&self.file)?;
        Ok(Frame {
            width,
            height,
            pixels,
        })
    }
}

pub fn measure(report: &Path, files: impl IntoIterator<Item = PathBuf>) -> PdfResult<()> {
    let start = Instant::now();
    let engine = open_engine()?;
    let engine_ms = start.elapsed().as_secs_f64() * 1000.0;
    let mut out = String::from("file\toperazione\tpagina\tmillisecondi\n");
    writeln!(out, "-\tinizializzazione motore\t-\t{engine_ms:.2}")?;
    for path in files {
        let start = Instant::now();
        let score = Score::open(&engine, &path, 0)?;
        let file = path
            .file_name()
            .unwrap_or(path.as_os_str())
            .to_string_lossy();
        writeln!(
            out,
            "{file}\tapertura\t-\t{:.2}",
            start.elapsed().as_secs_f64() * 1000.0
        )?;
        if score.count > 0
            && let Some((width, height)) = preview_dimensions(1824, 2736)
        {
            let start = Instant::now();
            let _frame = score.render(0, width, height)?;
            writeln!(
                out,
                "{file}\tanteprima\t1\t{:.2}",
                start.elapsed().as_secs_f64() * 1000.0
            )?;
        }
        for index in 0..score.count.min(12) {
            let start = Instant::now();
            let _frame = score.render(index, 1824, 2736)?;
            writeln!(
                out,
                "{file}\trasterizzazione\t{}\t{:.2}",
                index + 1,
                start.elapsed().as_secs_f64() * 1000.0
            )?;
        }
    }
    std::fs::write(report, out)?;
    Ok(())
}
