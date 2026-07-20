//! A Wadler/Prettier-style document algebra and best-fit printer.
//!
//! A [`Doc`] describes *possible* layouts of text; the printer chooses concrete
//! line breaks so that [`Group`](DocKind::Group)s stay on one line when they fit
//! within the target width and break onto multiple lines when they do not. This
//! engine is deliberately independent of Waluau's grammar: the CST-to-`Doc`
//! lowering lives elsewhere and only assembles these primitives.
//!
//! The layout algorithm follows Prettier's: a stack of `(indent, mode, doc)`
//! commands is processed left to right while tracking the current column. A
//! group is printed flat when a bounded look-ahead ([`fits`]) shows its flat
//! form reaches the next hard break without exceeding the width; otherwise it
//! breaks, and its `Line`s become newlines.

use std::rc::Rc;

/// An immutable, cheaply cloneable layout document. Construct via the free
/// functions ([`text`], [`concat`], [`group`], …) rather than by hand.
#[derive(Clone)]
pub struct Doc(Rc<DocKind>);

enum DocKind {
    /// Empty; prints nothing.
    Nil,
    /// Literal text. Must not contain `\n` — use [`hardline`] for newlines.
    Text(String),
    /// A space when flat, a newline (+ indent) when its group breaks.
    Line,
    /// Nothing when flat, a newline (+ indent) when its group breaks.
    SoftLine,
    /// Always a newline (+ indent); forces every enclosing group to break.
    HardLine,
    /// A sequence of documents printed back to back.
    Concat(Vec<Doc>),
    /// Increases the indentation applied to newlines inside `.0`.
    Indent(Doc),
    /// A layout choice point: printed flat if it fits, otherwise broken.
    Group(Doc),
}

/// The empty document.
pub fn nil() -> Doc {
    Doc(Rc::new(DocKind::Nil))
}

/// Literal text. Panics in debug if `s` contains a newline.
pub fn text(s: impl Into<String>) -> Doc {
    let s = s.into();
    debug_assert!(!s.contains('\n'), "Doc::text must not contain newlines");
    Doc(Rc::new(DocKind::Text(s)))
}

/// A space when flat, a newline when the enclosing group breaks.
pub fn line() -> Doc {
    Doc(Rc::new(DocKind::Line))
}

/// Nothing when flat, a newline when the enclosing group breaks.
pub fn softline() -> Doc {
    Doc(Rc::new(DocKind::SoftLine))
}

/// An unconditional newline that forces enclosing groups to break.
pub fn hardline() -> Doc {
    Doc(Rc::new(DocKind::HardLine))
}

/// Concatenate documents.
pub fn concat(parts: impl IntoIterator<Item = Doc>) -> Doc {
    Doc(Rc::new(DocKind::Concat(parts.into_iter().collect())))
}

/// Indent newlines inside `doc` by one further level.
pub fn indent(doc: Doc) -> Doc {
    Doc(Rc::new(DocKind::Indent(doc)))
}

/// A layout choice point. Printed on one line when it fits the target width
/// (and contains no hard break), otherwise broken across lines.
pub fn group(doc: Doc) -> Doc {
    Doc(Rc::new(DocKind::Group(doc)))
}

/// Join `items` with `sep` between each adjacent pair.
pub fn join(sep: Doc, items: impl IntoIterator<Item = Doc>) -> Doc {
    let mut out = Vec::new();
    for (i, item) in items.into_iter().enumerate() {
        if i > 0 {
            out.push(sep.clone());
        }
        out.push(item);
    }
    concat(out)
}

impl Doc {
    /// Whether this document contains a hard break, which forces any enclosing
    /// group to break. Descends through groups: a hard break anywhere inside
    /// propagates outward, matching Prettier's break propagation.
    fn has_forced_break(&self) -> bool {
        match &*self.0 {
            DocKind::HardLine => true,
            DocKind::Text(_) | DocKind::Nil | DocKind::Line | DocKind::SoftLine => false,
            DocKind::Indent(d) | DocKind::Group(d) => d.has_forced_break(),
            DocKind::Concat(parts) => parts.iter().any(Doc::has_forced_break),
        }
    }
}

/// Options controlling layout.
#[derive(Clone, Copy, Debug)]
pub struct PrintOptions {
    /// Target maximum line width in columns.
    pub width: usize,
    /// Number of spaces per indentation level.
    pub indent: usize,
}

impl Default for PrintOptions {
    fn default() -> Self {
        PrintOptions {
            width: 100,
            indent: 4,
        }
    }
}

#[derive(Clone, Copy, PartialEq)]
enum Mode {
    Flat,
    Break,
}

/// A pending unit of work: print `doc` at the given indentation and mode.
struct Cmd {
    indent: usize,
    mode: Mode,
    doc: Doc,
}

/// Render `doc` to a string under `opts`.
pub fn print(doc: &Doc, opts: PrintOptions) -> String {
    let mut out = String::new();
    let mut pos = 0usize;
    // Command stack processed from the back (the front of the document is the
    // top of the stack).
    let mut cmds: Vec<Cmd> = vec![Cmd {
        indent: 0,
        mode: Mode::Break,
        doc: doc.clone(),
    }];

    while let Some(Cmd { indent, mode, doc }) = cmds.pop() {
        match &*doc.0 {
            DocKind::Nil => {}
            DocKind::Text(s) => {
                out.push_str(s);
                pos += s.chars().count();
            }
            DocKind::Concat(parts) => {
                for part in parts.iter().rev() {
                    cmds.push(Cmd {
                        indent,
                        mode,
                        doc: part.clone(),
                    });
                }
            }
            DocKind::Indent(inner) => cmds.push(Cmd {
                indent: indent + opts.indent,
                mode,
                doc: inner.clone(),
            }),
            DocKind::Line => match mode {
                Mode::Flat => {
                    out.push(' ');
                    pos += 1;
                }
                Mode::Break => pos = newline(&mut out, indent),
            },
            DocKind::SoftLine => {
                if mode == Mode::Break {
                    pos = newline(&mut out, indent);
                }
            }
            DocKind::HardLine => pos = newline(&mut out, indent),
            DocKind::Group(inner) => {
                let flat_fits = !inner.has_forced_break()
                    && fits(
                        opts.width.saturating_sub(pos),
                        Cmd {
                            indent,
                            mode: Mode::Flat,
                            doc: inner.clone(),
                        },
                        &cmds,
                        opts,
                    );
                cmds.push(Cmd {
                    indent,
                    mode: if flat_fits { Mode::Flat } else { Mode::Break },
                    doc: inner.clone(),
                });
            }
        }
    }

    out
}

/// Emit a newline followed by `indent` spaces, and return the new column.
fn newline(out: &mut String, indent: usize) -> usize {
    // Trim trailing spaces on the line we are leaving so blank/indent-only
    // lines don't carry whitespace.
    let trimmed = out.trim_end_matches(' ').len();
    out.truncate(trimmed);
    out.push('\n');
    for _ in 0..indent {
        out.push(' ');
    }
    indent
}

/// Bounded look-ahead: does printing `next` (then the already-queued `rest`)
/// stay within `remaining` columns before the next line break? Groups and
/// lines are measured in flat mode; a break-mode line or a hard line ends the
/// measured segment and counts as fitting.
fn fits(remaining: usize, next: Cmd, rest: &[Cmd], opts: PrintOptions) -> bool {
    let mut remaining = remaining as isize;
    // Local stack seeded with `next`; `rest` is consumed from its back
    // (its top) only after the local stack drains.
    let mut stack = vec![next];
    let mut rest_idx = rest.len();

    loop {
        if remaining < 0 {
            return false;
        }
        let Cmd { indent, mode, doc } = match stack.pop() {
            Some(cmd) => cmd,
            None => {
                if rest_idx == 0 {
                    return true;
                }
                rest_idx -= 1;
                // `rest` is in stack order (back = next to run); clone so we
                // don't disturb the real command stack.
                Cmd {
                    indent: rest[rest_idx].indent,
                    mode: rest[rest_idx].mode,
                    doc: rest[rest_idx].doc.clone(),
                }
            }
        };
        match &*doc.0 {
            DocKind::Nil => {}
            DocKind::Text(s) => remaining -= s.chars().count() as isize,
            DocKind::Concat(parts) => {
                for part in parts.iter().rev() {
                    stack.push(Cmd {
                        indent,
                        mode,
                        doc: part.clone(),
                    });
                }
            }
            DocKind::Indent(inner) => stack.push(Cmd {
                indent: indent + opts.indent,
                mode,
                doc: inner.clone(),
            }),
            DocKind::Group(inner) => stack.push(Cmd {
                indent,
                // Nested groups are measured flat while probing an outer group.
                mode: Mode::Flat,
                doc: inner.clone(),
            }),
            DocKind::Line => match mode {
                Mode::Flat => remaining -= 1,
                // A break here ends the current line: everything so far fit.
                Mode::Break => return true,
            },
            DocKind::SoftLine => {
                if mode == Mode::Break {
                    return true;
                }
            }
            // A hard line always ends the current line.
            DocKind::HardLine => return true,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn print80(doc: &Doc) -> String {
        print(
            doc,
            PrintOptions {
                width: 80,
                indent: 2,
            },
        )
    }

    #[test]
    fn group_stays_flat_when_it_fits() {
        let doc = group(concat([
            text("["),
            indent(concat([
                softline(),
                text("a"),
                text(","),
                line(),
                text("b"),
            ])),
            softline(),
            text("]"),
        ]));
        assert_eq!(print80(&doc), "[a, b]");
    }

    #[test]
    fn group_breaks_when_too_wide() {
        let doc = print(
            &group(concat([
                text("["),
                indent(concat([
                    softline(),
                    text("aaaa"),
                    text(","),
                    line(),
                    text("bbbb"),
                ])),
                softline(),
                text("]"),
            ])),
            PrintOptions {
                width: 8,
                indent: 2,
            },
        );
        assert_eq!(doc, "[\n  aaaa,\n  bbbb\n]");
    }

    #[test]
    fn hardline_forces_enclosing_group_to_break() {
        let doc = group(concat([text("do"), hardline(), text("end")]));
        assert_eq!(print80(&doc), "do\nend");
    }

    #[test]
    fn nested_indentation_accumulates() {
        let inner = concat([text("x"), hardline(), text("y")]);
        let doc = concat([text("a"), indent(concat([hardline(), indent(inner)]))]);
        assert_eq!(print80(&doc), "a\n  x\n    y");
    }

    #[test]
    fn newline_trims_trailing_spaces() {
        // A `line` that breaks should not leave a trailing space behind.
        let doc = group(concat([text("a"), hardline(), text("b")]));
        let out = print80(&doc);
        assert!(!out.contains(" \n"), "unexpected trailing space: {out:?}");
    }

    #[test]
    fn join_places_separators_between_items() {
        let doc = group(join(
            concat([text(","), line()]),
            [text("a"), text("b"), text("c")],
        ));
        assert_eq!(print80(&doc), "a, b, c");
    }
}
