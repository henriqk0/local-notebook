from __future__ import annotations

from pathlib import Path


def estimate_tokens(text: str) -> int:
    # Simple heuristic: ~4 chars per token
    return len(text) // 4 if text else 0


def read_text_file(path: Path, max_size_mb: int = 20) -> tuple[str, int, str | None]:
    try:
        size_bytes = path.stat().st_size
    except OSError:
        size_bytes = 0
    max_bytes = max_size_mb * 1024 * 1024
    if size_bytes > max_bytes:
        raise ValueError(f"File larger than {max_size_mb}MB: {path}")

    try:
        with path.open("r", encoding="utf-8", errors="replace") as f:
            content = f.read()
        return content, size_bytes, None
    except (OSError, UnicodeError) as e:
        return "", size_bytes, f"Failed to read file: {e}"


def extract_text(path: Path) -> tuple[str, int, str | None, str | None, str | None]:
    # Determine extension/mime
    ext = path.suffix.lower().lstrip(".")
    mime = None

    if ext in ("txt", "md", "markdown", "rst", "log", "json", "yaml", "yml", "csv", "tsv", "py", "js", "ts", "java", "cpp", "c", "h", "go", "rs", "toml", "ini", "sh"):
        try:
            content, size, err = read_text_file(path)
            return content, size, ext, mime, err
        except ValueError as e:
            try:
                size = path.stat().st_size
            except OSError:
                size = 0
            return "", size, ext, mime, str(e)
    elif ext == "pdf":
        try:
            import pypdf  # type: ignore

            content_parts: list[str] = []
            with path.open("rb") as f:
                reader = pypdf.PdfReader(f)
                for page in reader.pages:
                    try:
                        text = page.extract_text() or ""
                    except (ValueError, RuntimeError, AttributeError, TypeError):
                        text = ""
                    content_parts.append(text)
            content = "\n\n".join(content_parts)
            try:
                size = path.stat().st_size
            except OSError:
                size = 0
            return content, size, ext, "application/pdf", None
        except ImportError:
            try:
                size = path.stat().st_size
            except OSError:
                size = 0
            return "", size, ext, "application/pdf", "pypdf not available"
        except (OSError, RuntimeError, ValueError) as e:
            try:
                size = path.stat().st_size
            except OSError:
                size = 0
            return "", size, ext, "application/pdf", str(e)
    elif ext in ("docx",):
        try:
            from docx import Document  # type: ignore

            doc = Document(str(path))
            parts: list[str] = []
            for para in doc.paragraphs:
                parts.append(para.text)
            content = "\n\n".join(parts)
            try:
                size = path.stat().st_size
            except OSError:
                size = 0
            return content, size, ext, "application/vnd.openxmlformats-officedocument.wordprocessingml.document", None
        except ImportError:
            try:
                size = path.stat().st_size
            except OSError:
                size = 0
            return "", size, ext, "application/vnd.openxmlformats-officedocument.wordprocessingml.document", "python-docx not available"
        except (OSError, RuntimeError, ValueError) as e:
            try:
                size = path.stat().st_size
            except OSError:
                size = 0
            return "", size, ext, "application/vnd.openxmlformats-officedocument.wordprocessingml.document", str(e)
    elif ext in ("html", "htm", "xhtml"):
        try:
            with path.open("r", encoding="utf-8", errors="replace") as f:
                html = f.read()
            from bs4 import BeautifulSoup  # type: ignore

            soup = BeautifulSoup(html, "html.parser")
            for script in soup(["script", "style"]):
                script.extract()
            content = soup.get_text(separator="\n")
            try:
                size = path.stat().st_size
            except OSError:
                size = 0
            return content, size, ext, "text/html", None
        except (OSError, UnicodeError, ValueError) as e:
            try:
                size = path.stat().st_size
            except OSError:
                size = 0
            return "", size, ext, "text/html", str(e)
    else:
        try:
            size = path.stat().st_size
        except OSError:
            size = 0
        return "", size, ext or None, None, "Unsupported or binary file type"
