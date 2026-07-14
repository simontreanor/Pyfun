;;; pyfun-mode.el --- Major mode for the Pyfun language -*- lexical-binding: t; -*-

;; Copyright (C) 2026 Simon Treanor

;; Author: Simon Treanor
;; Maintainer: Simon Treanor
;; URL: https://github.com/simontreanor/Pyfun
;; Version: 0.1.0
;; Package-Requires: ((emacs "29.1"))
;; Keywords: languages

;; This file is not part of GNU Emacs.

;; Licensed under the Apache License, Version 2.0 (the "License"); you may
;; not use this file except in compliance with the License.  You may obtain
;; a copy of the License at http://www.apache.org/licenses/LICENSE-2.0

;;; Commentary:

;; A major mode for Pyfun (https://github.com/simontreanor/Pyfun), an
;; F#-inspired, functional-first language that compiles to readable Python.
;;
;; Provides syntax highlighting, comment support, and (on Emacs 29+) eglot
;; integration with the language server bundled in the Pyfun compiler:
;; install it with `pip install pyfun-lang', which puts `pyfun' on PATH, and
;; run M-x eglot in a Pyfun buffer (or enable `eglot-ensure' via
;; `pyfun-mode-hook') for diagnostics, hover types and effects,
;; go-to-definition, rename, and completion.

;;; Code:

(defgroup pyfun nil
  "Support for the Pyfun language."
  :group 'languages
  :prefix "pyfun-")

(defconst pyfun-mode--keywords
  '("let" "mut" "pure" "if" "then" "else" "elif" "match" "case" "with"
    "fun" "type" "return" "yield" "do" "measure" "extern" "module"
    "import" "try" "as" "not" "and" "or")
  "Pyfun reserved words.")

(defconst pyfun-mode--font-lock-keywords
  `((,(regexp-opt pyfun-mode--keywords 'symbols) . font-lock-keyword-face)
    ("\\_<\\(?:true\\|false\\)\\_>" . font-lock-constant-face)
    ;; Uppercase-initial identifiers: types, constructors, modules.
    ("\\_<[A-Z][A-Za-z0-9_]*\\_>" . font-lock-type-face)
    ;; A `let' binding's name (function or value).
    ("\\_<let\\_>\\(?:\\s-+\\(?:mut\\|pure\\)\\)*\\s-+\\([a-z_][A-Za-z0-9_]*\\)"
     (1 font-lock-function-name-face))
    ;; Typed holes.
    ("\\?[A-Za-z_][A-Za-z0-9_]*\\|\\?" . font-lock-warning-face))
  "Font-lock rules for `pyfun-mode'.")

(defvar pyfun-mode-syntax-table
  (let ((table (make-syntax-table)))
    (modify-syntax-entry ?# "<" table)   ; comments: # to end of line
    (modify-syntax-entry ?\n ">" table)
    (modify-syntax-entry ?\" "\"" table) ; strings
    (modify-syntax-entry ?\\ "\\" table)
    (modify-syntax-entry ?_ "_" table)
    (modify-syntax-entry ?' "." table)
    table)
  "Syntax table for `pyfun-mode'.")

;;;###autoload
(define-derived-mode pyfun-mode prog-mode "Pyfun"
  "Major mode for editing Pyfun source files.

Pyfun is an F#-inspired, functional-first language that compiles
to readable Python.  For IDE features, install the compiler
\(`pip install pyfun-lang') and use eglot: the `pyfun lsp' server
is registered automatically when eglot loads."
  (setq-local comment-start "# ")
  (setq-local comment-start-skip "#+\\s-*")
  (setq-local comment-end "")
  (setq-local indent-tabs-mode nil)
  (setq-local tab-width 4)
  (setq-local font-lock-defaults '(pyfun-mode--font-lock-keywords)))

;;;###autoload
(add-to-list 'auto-mode-alist '("\\.pyfun\\'" . pyfun-mode))

(defvar eglot-server-programs)
(with-eval-after-load 'eglot
  (add-to-list 'eglot-server-programs '(pyfun-mode . ("pyfun" "lsp"))))

(provide 'pyfun-mode)

;;; pyfun-mode.el ends here
