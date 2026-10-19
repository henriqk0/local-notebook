from __future__ import annotations

import zipfile
from pathlib import Path

from ..tokens import estimate_tokens  # noqa: F401  (re-exported for compatibility)

MAX_FILE_SIZE_MB = 20
MAX_FILE_SIZE_BYTES = MAX_FILE_SIZE_MB * 1024 * 1024
MAX_DECOMPRESSED_BYTES = 100 * 1024 * 1024
MAX_PDF_PAGES = 1000

TEXT_EXTS = (
    "txt",
    "md",
    "markdown",
    "rst",
    "log",
    "json",
    "yaml",
    "yml",
    "csv",
    "tsv",
    "py",
    "js",
    "ts",
    "java",
    "cpp",
    "c",
    "h",
    "go",
    "rs",
    "toml",
    "ini",
    "sh",
)


def _file_size(path: Path) -> int:
    try:
        return path.stat().st_size
    except OSError:
        return 0


def read_text_file(
    path: Path, max_size_mb: int = MAX_FILE_SIZE_MB
) -> tuple[str, int, str | None]:
    size_bytes = _file_size(path)
    max_bytes = max_size_mb * 1024 * 1024
    if size_bytes > max_bytes:
        raise ValueError(f"File larger than {max_size_mb}MB: {path}")

    try:
        with path.open("r", encoding="utf-8", errors="replace") as f:
            content = f.read()
        return content, size_bytes, None
    except (OSError, UnicodeError) as e:
        return "", size_bytes, f"Failed to read file: {e}"


def _read_html(path: Path) -> tuple[str, str | None]:
    try:
        html, _size, err = read_text_file(path)
        if err is not None:
            return "", err
        from bs4 import BeautifulSoup  # type: ignore

        soup = BeautifulSoup(html, "html.parser")
        for script in soup(["script", "style"]):
            script.extract()
        return soup.get_text(separator="\n"), None
    except (OSError, UnicodeError, ValueError) as e:
        return "", str(e)


def _read_pdf(path: Path) -> tuple[str, str | None]:
    try:
        import pypdf  # type: ignore

        content_parts: list[str] = []
        with path.open("rb") as f:
            reader = pypdf.PdfReader(f)
            if len(reader.pages) > MAX_PDF_PAGES:
                return "", f"PDF has more than {MAX_PDF_PAGES} pages"
            for page in reader.pages:
                try:
                    text = page.extract_text() or ""
                except Exception:  # noqa: BLE001 - one bad page shouldn't fail the file
                    text = ""
                content_parts.append(text)
        return "\n\n".join(content_parts), None
    except ImportError:
        return "", "pypdf not available"
    except Exception as e:  # noqa: BLE001 - pypdf raises several unrelated errors
        return "", f"Failed to read PDF: {e}"


def _read_docx(path: Path) -> tuple[str, str | None]:
    try:
        # Check compressed and decompressed size before handing the file to
        # python-docx so zip bombs cannot exhaust memory.
        with zipfile.ZipFile(path) as zf:
            total_uncompressed = sum(info.file_size for info in zf.infolist())
            if total_uncompressed > MAX_DECOMPRESSED_BYTES:
                return "", "DOCX expands beyond the decompressed size limit"

        from docx import Document  # type: ignore

        doc = Document(str(path))
        parts = [para.text for para in doc.paragraphs]
        return "\n\n".join(parts), None
    except ImportError:
        return "", "python-docx not available"
    except Exception as e:  # noqa: BLE001 - docx/bad zip raise several error types
        return "", f"Failed to read DOCX: {e}"


def extract_text(path: Path) -> tuple[str, int, str | None, str | None, str | None]:
    ext = path.suffix.lower().lstrip(".")
    size = _file_size(path)

    if size > MAX_FILE_SIZE_BYTES:
        return "", size, ext or None, None, f"File larger than {MAX_FILE_SIZE_MB}MB"

    if ext in TEXT_EXTS:
        try:
            content, size, err = read_text_file(path)
            return content, size, ext, "text/plain", err
        except ValueError as e:
            return "", size, ext, "text/plain", str(e)

    if ext == "pdf":
        content, err = _read_pdf(path)
        return content, size, ext, "application/pdf", err

    if ext == "docx":
        content, err = _read_docx(path)
        return (
            content,
            size,
            ext,
            "application/vnd.openxmlformats-officedocument.wordprocessingml.document",
            err,
        )

    if ext in ("html", "htm", "xhtml"):
        content, err = _read_html(path)
        return content, size, ext, "text/html", err

    return "", size, ext or None, None, "Unsupported or binary file type"
