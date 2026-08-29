// External scanner for the Waluau tree-sitter grammar.
//
// Handles the two token families that need unbounded bracket-level matching,
// which tree-sitter's regex-based lexer cannot express:
//
//   comment      `-- ...` line comments and `--[[ ... ]]` block comments,
//                including the leveled `--[=*[ ... ]=*]` form
//   long_string  `[[ ... ]]` long strings, including the leveled
//                `[=*[ ... ]=*]` form
//
// The lexing rules mirror crates/waluau-lexer: a block comment or long
// string opened with `[` + N `=` + `[` closes only at `]` + N `=` + `]`.
// An unterminated block runs to end of file and is still produced as a
// token so the editor colors the trailing region.

#include "tree_sitter/parser.h"

#include <wctype.h>

enum TokenType {
  COMMENT,
  LONG_STRING,
  CONST_KEYWORD,
};

static bool is_identifier_char(int32_t c) {
  return (c >= 'a' && c <= 'z') || (c >= 'A' && c <= 'Z') ||
         (c >= '0' && c <= '9') || c == '_';
}

void *tree_sitter_waluau_external_scanner_create(void) { return NULL; }

void tree_sitter_waluau_external_scanner_destroy(void *payload) { (void)payload; }

unsigned tree_sitter_waluau_external_scanner_serialize(void *payload, char *buffer) {
  (void)payload;
  (void)buffer;
  return 0;
}

void tree_sitter_waluau_external_scanner_deserialize(void *payload, const char *buffer,
                                                     unsigned length) {
  (void)payload;
  (void)buffer;
  (void)length;
}

static void advance(TSLexer *lexer) { lexer->advance(lexer, false); }

// With the lookahead at the first `[` of a candidate long bracket, consume
// `[` + N `=` and, if a second `[` follows, the whole bracketed region.
// Returns true when a complete (or EOF-truncated) long bracket was consumed.
static bool scan_long_bracket(TSLexer *lexer) {
  advance(lexer); // first '['
  unsigned level = 0;
  while (lexer->lookahead == '=') {
    level++;
    advance(lexer);
  }
  if (lexer->lookahead != '[') {
    return false;
  }
  advance(lexer); // second '['
  for (;;) {
    if (lexer->eof(lexer)) {
      // Unterminated: claim the rest of the file, like the compiler's
      // diagnostic region.
      return true;
    }
    if (lexer->lookahead == ']') {
      advance(lexer);
      unsigned eq = 0;
      while (lexer->lookahead == '=') {
        eq++;
        advance(lexer);
      }
      if (eq == level && lexer->lookahead == ']') {
        advance(lexer);
        return true;
      }
    } else {
      advance(lexer);
    }
  }
}

bool tree_sitter_waluau_external_scanner_scan(void *payload, TSLexer *lexer,
                                              const bool *valid_symbols) {
  (void)payload;
  if (!valid_symbols[COMMENT] && !valid_symbols[LONG_STRING] &&
      !valid_symbols[CONST_KEYWORD]) {
    return false;
  }

  while (iswspace((wint_t)lexer->lookahead)) {
    lexer->advance(lexer, true);
  }

  if (valid_symbols[COMMENT] && lexer->lookahead == '-') {
    advance(lexer);
    if (lexer->lookahead != '-') {
      // A lone '-' is the minus operator; failing here resets the position.
      return false;
    }
    advance(lexer);
    lexer->result_symbol = COMMENT;
    if (lexer->lookahead == '[' && scan_long_bracket(lexer)) {
      return true;
    }
    // Line comment; `--[` without a matching second bracket also lands here
    // and correctly runs to the end of the line.
    while (lexer->lookahead != '\n' && !lexer->eof(lexer)) {
      advance(lexer);
    }
    return true;
  }

  if (valid_symbols[LONG_STRING] && lexer->lookahead == '[') {
    if (scan_long_bracket(lexer)) {
      lexer->result_symbol = LONG_STRING;
      return true;
    }
    // Plain '[' indexing; failing resets so the internal lexer handles it.
    return false;
  }

  // `const` is contextual (crates/waluau-parser parse_const_decl): it opens
  // a declaration only when the next token is an identifier or `function`,
  // both of which start with an identifier character. `const = 1` and
  // `const(x)` keep parsing as ordinary statements about a variable named
  // `const`.
  if (valid_symbols[CONST_KEYWORD] && lexer->lookahead == 'c') {
    static const char keyword[] = "const";
    for (const char *c = keyword; *c != '\0'; c++) {
      if (lexer->lookahead != *c) {
        return false;
      }
      advance(lexer);
    }
    if (is_identifier_char(lexer->lookahead)) {
      // A longer identifier such as `constant`.
      return false;
    }
    // The token ends here; everything after is pure lookahead (and must not
    // use skip=true, which would reset the token's start position).
    lexer->mark_end(lexer);
    while (iswspace((wint_t)lexer->lookahead)) {
      advance(lexer);
    }
    if (is_identifier_char(lexer->lookahead)) {
      lexer->result_symbol = CONST_KEYWORD;
      return true;
    }
    return false;
  }

  return false;
}
