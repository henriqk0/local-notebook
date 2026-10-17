from __future__ import annotations


def chunk_text(text: str, chunk_tokens: int = 800, overlap_tokens: int = 100) -> list[str]:
    if not text:
        return []

    # Simple char-based approximation to tokens (chars/4)
    chars_per_token = 4
    chunk_chars = chunk_tokens * chars_per_token
    overlap_chars = overlap_tokens * chars_per_token

    # Split on paragraph boundaries first
    paragraphs = text.split("\n\n")
    chunks: list[str] = []
    current = ""

    for para in paragraphs:
        # If adding this paragraph makes it too long, flush current
        if len(current) + len(para) + 2 > chunk_chars and current:
            chunks.append(current.strip())
            # Start new with overlap from end
            if overlap_chars > 0 and len(current) > overlap_chars:
                current = current[-overlap_chars:] + "\n\n" + para
            else:
                current = para
        else:
            current = (current + "\n\n" + para).strip() if current else para
    if current:
        chunks.append(current.strip())

    # Fallback if no chunking happened (very short or no paragraphs)
    if len(chunks) == 0 and text:
        # Hard cut
        for i in range(0, len(text), chunk_chars - overlap_chars):
            chunk = text[i : i + chunk_chars]
            if chunk:
                chunks.append(chunk)
            if i + chunk_chars >= len(text):
                break

    # Dedup very similar consecutive empty-ish chunks? Keep simple
    return [c for c in chunks if c]
