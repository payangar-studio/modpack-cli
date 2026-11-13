//! Blocks between markers in a file the pack owns otherwise (`AGENTS.md`,
//! `.gitignore`). The socle only ever reads and writes the lines between its
//! markers; the rest of the file is left byte for byte as it was.

/// The begin and end lines around a block, in the comment syntax of the file.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Markers {
    pub begin: &'static str,
    pub end: &'static str,
}

/// Markers of a Markdown file.
pub const HTML: Markers = Markers {
    begin: "<!-- modpack:begin -->",
    end: "<!-- modpack:end -->",
};

/// Markers of a file whose comments start with `#`.
pub const HASH: Markers = Markers {
    begin: "# modpack:begin",
    end: "# modpack:end",
};

/// Byte range of the block's content: from the line after the begin marker to
/// the start of the end marker line.
fn content_range(text: &str, markers: Markers) -> Option<(usize, usize)> {
    let mut start = None;
    let mut offset = 0;
    for line in text.split_inclusive('\n') {
        let trimmed = line.trim();
        match start {
            None if trimmed == markers.begin => start = Some(offset + line.len()),
            Some(start) if trimmed == markers.end => return Some((start, offset)),
            _ => {}
        }
        offset += line.len();
    }
    None
}

/// The block's content as it stands in `text`, if both markers are there.
pub fn find(text: &str, markers: Markers) -> Option<&str> {
    content_range(text, markers).map(|(start, end)| &text[start..end])
}

/// `text` with the block's content set to `content`. A text without the block
/// gets it at the end, after a blank line when the text is not empty.
pub fn upsert(text: &str, markers: Markers, content: &str) -> String {
    let content = with_final_newline(content);
    match content_range(text, markers) {
        Some((start, end)) => format!("{}{content}{}", &text[..start], &text[end..]),
        None => {
            let mut out = text.to_owned();
            if !out.is_empty() {
                if !out.ends_with('\n') {
                    out.push('\n');
                }
                out.push('\n');
            }
            out.push_str(&format!("{}\n{content}{}\n", markers.begin, markers.end));
            out
        }
    }
}

/// `text` without the block, markers included. Unchanged when the block is
/// not there.
pub fn remove(text: &str, markers: Markers) -> String {
    let Some((start, end)) = content_range(text, markers) else {
        return text.to_owned();
    };
    let begin_line = text[..start]
        .trim_end_matches('\n')
        .rfind('\n')
        .map_or(0, |i| i + 1);
    let end_line = text[end..].find('\n').map_or(text.len(), |i| end + i + 1);
    format!("{}{}", &text[..begin_line], &text[end_line..])
}

/// The content as the socle writes it, so that its hash is stable.
pub fn with_final_newline(content: &str) -> String {
    if content.is_empty() || content.ends_with('\n') {
        content.to_owned()
    } else {
        format!("{content}\n")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const AROUND: &str = "# Pack\n\nOwn text.\n\n<!-- modpack:begin -->\nold\n<!-- modpack:end -->\n\nMore own text.\n";

    #[test]
    fn finds_the_content_between_markers() {
        assert_eq!(find(AROUND, HTML), Some("old\n"));
        assert_eq!(find("no block\n", HTML), None);
        assert_eq!(find("<!-- modpack:begin -->\nunterminated\n", HTML), None);
    }

    #[test]
    fn replaces_only_the_content_between_markers() {
        let out = upsert(AROUND, HTML, "new\nlines");
        assert_eq!(
            out,
            "# Pack\n\nOwn text.\n\n<!-- modpack:begin -->\nnew\nlines\n<!-- modpack:end -->\n\nMore own text.\n"
        );
        assert_eq!(upsert(&out, HTML, "new\nlines\n"), out);
    }

    #[test]
    fn appends_the_block_to_a_file_without_it() {
        assert_eq!(
            upsert("target/\n", HASH, "mods/*.jar\n"),
            "target/\n\n# modpack:begin\nmods/*.jar\n# modpack:end\n"
        );
        assert_eq!(
            upsert("no final newline", HASH, "x\n"),
            "no final newline\n\n# modpack:begin\nx\n# modpack:end\n"
        );
    }

    #[test]
    fn creates_the_block_in_an_empty_file() {
        assert_eq!(
            upsert("", HASH, "x\n"),
            "# modpack:begin\nx\n# modpack:end\n"
        );
    }

    #[test]
    fn markers_of_another_syntax_are_ignored() {
        let text = "# modpack:begin\nx\n# modpack:end\n";
        assert_eq!(find(text, HTML), None);
    }

    #[test]
    fn crlf_lines_are_recognised() {
        let text = "a\r\n# modpack:begin\r\nx\r\n# modpack:end\r\nb\r\n";
        assert_eq!(find(text, HASH), Some("x\r\n"));
    }

    #[test]
    fn removes_the_block_and_its_markers() {
        assert_eq!(
            remove(AROUND, HTML),
            "# Pack\n\nOwn text.\n\n\nMore own text.\n"
        );
        assert_eq!(remove("nothing\n", HTML), "nothing\n");
        assert_eq!(remove("# modpack:begin\nx\n# modpack:end", HASH), "");
    }
}
