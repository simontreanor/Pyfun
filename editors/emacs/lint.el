;;; lint.el --- CI checks for pyfun-mode.el -*- lexical-binding: t; -*-

;; Run from the repository root:
;;
;;   emacs -Q --batch -l editors/emacs/lint.el
;;
;; Byte-compiles pyfun-mode.el with warnings as errors, then runs checkdoc
;; and package-lint over it, the same checks MELPA reviewers run.  Exits
;; non-zero if any of them reports a problem.  Not part of the MELPA package:
;; the recipe's :files spec selects pyfun-mode.el alone.

;;; Code:

(require 'package)
(require 'checkdoc)

(defconst pyfun-lint--file
  (expand-file-name "editors/emacs/pyfun-mode.el" default-directory))

(defconst pyfun-lint--allowed
  ;; The eglot registration is deliberate, and was accepted in MELPA review
  ;; (melpa/melpa#10189): it only tells eglot which server to start for
  ;; pyfun-mode buffers, and nothing happens for anyone who never loads eglot.
  '("`with-eval-after-load' is for use in configurations")
  "Substrings of package-lint messages that do not fail the check.")

(defvar pyfun-lint--failed nil)

(defun pyfun-lint--fail (fmt &rest args)
  "Report a problem (FMT and ARGS as for `format') and mark the run failed."
  (setq pyfun-lint--failed t)
  (message "%s" (apply #'format fmt args)))

;; Byte-compile, into a scratch directory so the tree stays clean.
(message "== byte-compile")
(let* ((dir (make-temp-file "pyfun-lint" t))
       (byte-compile-dest-file-function
        (lambda (src) (expand-file-name (file-name-nondirectory (concat src "c")) dir)))
       (byte-compile-error-on-warn t))
  (unless (byte-compile-file pyfun-lint--file)
    (pyfun-lint--fail "byte-compile failed")))

;; checkdoc.  In batch mode it reports each problem through `checkdoc-error'
;; and carries on, so count the calls.
(message "== checkdoc")
(advice-add 'checkdoc-error :before
            (lambda (&rest _) (setq pyfun-lint--failed t)))
(checkdoc-file pyfun-lint--file)

;; package-lint, installed from MELPA into a throwaway package directory.
(message "== package-lint")
(setq package-user-dir (make-temp-file "pyfun-lint-elpa" t))
(add-to-list 'package-archives '("melpa" . "https://melpa.org/packages/") t)
(package-initialize)
(package-refresh-contents)
(package-install 'package-lint)
(require 'package-lint)
(with-temp-buffer
  (insert-file-contents pyfun-lint--file)
  (setq buffer-file-name pyfun-lint--file)
  (emacs-lisp-mode)
  (dolist (issue (package-lint-buffer))
    (pcase-let ((`(,line ,col ,type ,msg) issue))
      (if (seq-some (lambda (ok) (string-search ok msg)) pyfun-lint--allowed)
          (message "%d:%d: %s (allowed): %s" line col type msg)
        (pyfun-lint--fail "%d:%d: %s: %s" line col type msg))))
  (set-buffer-modified-p nil))

(message (if pyfun-lint--failed "== FAILED" "== ok"))
(kill-emacs (if pyfun-lint--failed 1 0))

;;; lint.el ends here
