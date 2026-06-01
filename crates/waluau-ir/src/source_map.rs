pub(crate) fn resolve_span_to_line_and_text(source: &str, span: waluau_ast::Span) -> (usize, String) {
    let chars: Vec<char> = source.chars().collect();
    let start = span.start as usize;
    let end = span.end as usize;

    let line_number = chars[..start.min(chars.len())]
        .iter()
        .filter(|&&c| c == '\n')
        .count()
        + 1;

    let text: String = if start < chars.len() && end <= chars.len() && start <= end {
        chars[start..end].iter().collect()
    } else {
        String::new()
    };

    (line_number, text)
}
