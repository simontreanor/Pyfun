// External scanner for Pyfun's offside rule.
//
// Mirrors the reference lexer (src/lexer/mod.rs in the Pyfun repo), which
// emits three synthetic zero-width tokens:
//
//   INDENT — a block opens: the grammar offers `_indent` only right after a
//            block opener (`=` `->` `then` `else` `:`), so "is a block
//            pending" is encoded in valid_symbols, and the scanner only has
//            to check that the next line is deeper.
//   DEDENT — the line is shallower than the current block column; one DEDENT
//            per level, emitted across successive scanner calls.
//   SEP    — statement separator: next line at the same column AND its first
//            token can start a statement. Lines leading with an infix
//            operator, `|`, `.`, `,`, a closing bracket, a lone `_`, or one
//            of `then else elif with and or in` continue the current
//            statement instead (no SEP).
//
// All three tokens are zero-width, anchored *before* the line break
// (mark_end at entry; everything after is lookahead). Whitespace and
// comments are re-consumed by the internal lexer as extras, so comment-only
// and blank lines are layout-transparent here. Brackets need no special
// handling: inside `()`/`{}`/`[]` the grammar never has a layout token
// valid, so the scanner declines and line breaks become plain whitespace —
// implicit continuation, exactly like the reference lexer's depth check.

#include <string.h>

#include "tree_sitter/alloc.h"
#include "tree_sitter/array.h"
#include "tree_sitter/parser.h"

enum TokenType {
    INDENT,
    DEDENT,
    SEP,
};

typedef struct {
    Array(uint16_t) indents;
} Scanner;

static inline void advance_pyfun(TSLexer *lexer) { lexer->advance(lexer, false); }

static inline bool is_ident_continue(int32_t c) {
    return c == '_' || (c >= '0' && c <= '9') || (c >= 'a' && c <= 'z') ||
           (c >= 'A' && c <= 'Z');
}

// Does the token at the lexer's current position start a new statement?
// (Reference: `upcoming_starts_stmt`, lexer/mod.rs.) May consume lookahead —
// only call once the layout decision needs it.
static bool starts_statement(TSLexer *lexer) {
    int32_t c = lexer->lookahead;
    switch (c) {
        // Infix-operator leads, `|`, `.`, `,`, and closing brackets continue
        // the previous statement.
        case '+':
        case '-':
        case '*':
        case '/':
        case '%':
        case '<':
        case '>':
        case '=':
        case '!':
        case '^':
        case '|':
        case '.':
        case ',':
        case ')':
        case ']':
        case '}':
            return false;
        default:
            break;
    }

    if (c == '_') {
        advance_pyfun(lexer);
        // A lone `_` continues; `_name` is an identifier and starts a
        // statement.
        return is_ident_continue(lexer->lookahead);
    }

    if (c >= 'a' && c <= 'z') {
        char word[8] = {0};
        unsigned len = 0;
        while (is_ident_continue(lexer->lookahead)) {
            if (len < sizeof(word) - 1) {
                word[len] = (char)lexer->lookahead;
            }
            len++;
            advance_pyfun(lexer);
            if (len >= sizeof(word)) {
                return true; // too long for any continuation keyword
            }
        }
        static const char *const continuation_keywords[] = {
            "then", "else", "elif", "with", "and", "or", "in",
        };
        for (unsigned i = 0;
             i < sizeof(continuation_keywords) / sizeof(*continuation_keywords);
             i++) {
            if (strcmp(word, continuation_keywords[i]) == 0) {
                return false;
            }
        }
    }

    return true;
}

static bool scan(Scanner *scanner, TSLexer *lexer, const bool *valid_symbols) {
    // In error recovery every symbol is marked valid; emitting SEP/INDENT
    // then would loop or mis-recover. Only allow block-closing DEDENTs.
    bool error_recovery =
        valid_symbols[INDENT] && valid_symbols[DEDENT] && valid_symbols[SEP];

    lexer->mark_end(lexer);

    bool found_end_of_line = false;
    bool at_eof = false;
    uint32_t indent = 0;

    for (;;) {
        int32_t c = lexer->lookahead;
        if (c == '\n') {
            found_end_of_line = true;
            indent = 0;
            advance_pyfun(lexer);
        } else if (c == '\r' || c == '\f') {
            indent = 0;
            advance_pyfun(lexer);
        } else if (c == ' ') {
            indent++;
            advance_pyfun(lexer);
        } else if (c == '\t') {
            indent += 8;
            advance_pyfun(lexer);
        } else if (c == '#') {
            if (!found_end_of_line) {
                // Trailing comment after code — the grammar's extras own it.
                return false;
            }
            while (!lexer->eof(lexer) && lexer->lookahead != '\n') {
                advance_pyfun(lexer);
            }
        } else if (lexer->eof(lexer)) {
            found_end_of_line = true;
            at_eof = true;
            indent = 0;
            break;
        } else {
            break;
        }
    }

    if (!found_end_of_line) {
        return false;
    }

    uint16_t current = *array_back(&scanner->indents);

    if (valid_symbols[DEDENT] && indent < current) {
        array_pop(&scanner->indents);
        lexer->result_symbol = DEDENT;
        return true;
    }

    if (error_recovery) {
        return false;
    }

    if (valid_symbols[INDENT] && indent > current) {
        if (scanner->indents.size < 400) {
            array_push(&scanner->indents, (uint16_t)indent);
        }
        lexer->result_symbol = INDENT;
        return true;
    }

    if (valid_symbols[SEP] && !at_eof && indent == current &&
        starts_statement(lexer)) {
        lexer->result_symbol = SEP;
        return true;
    }

    return false;
}

// ---- entry points ----

void *tree_sitter_pyfun_external_scanner_create(void) {
    Scanner *scanner = ts_calloc(1, sizeof(Scanner));
    array_init(&scanner->indents);
    array_push(&scanner->indents, 0);
    return scanner;
}

void tree_sitter_pyfun_external_scanner_destroy(void *payload) {
    Scanner *scanner = (Scanner *)payload;
    array_delete(&scanner->indents);
    ts_free(scanner);
}

unsigned tree_sitter_pyfun_external_scanner_serialize(void *payload, char *buffer) {
    Scanner *scanner = (Scanner *)payload;
    unsigned size = 0;
    // Skip the always-present base entry (0); re-added on deserialize.
    for (uint32_t i = 1; i < scanner->indents.size; i++) {
        uint16_t value = *array_get(&scanner->indents, i);
        if (size + 2 > TREE_SITTER_SERIALIZATION_BUFFER_SIZE) {
            break;
        }
        buffer[size++] = (char)(value & 0xFF);
        buffer[size++] = (char)((value >> 8) & 0xFF);
    }
    return size;
}

void tree_sitter_pyfun_external_scanner_deserialize(void *payload,
                                                    const char *buffer,
                                                    unsigned length) {
    Scanner *scanner = (Scanner *)payload;
    array_clear(&scanner->indents);
    array_push(&scanner->indents, 0);
    for (unsigned i = 0; i + 1 < length; i += 2) {
        uint16_t value = (uint16_t)((unsigned char)buffer[i] |
                                    ((unsigned char)buffer[i + 1] << 8));
        array_push(&scanner->indents, value);
    }
}

bool tree_sitter_pyfun_external_scanner_scan(void *payload, TSLexer *lexer,
                                             const bool *valid_symbols) {
    return scan((Scanner *)payload, lexer, valid_symbols);
}
