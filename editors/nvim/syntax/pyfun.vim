" Vim syntax file for Pyfun (https://github.com/simontreanor/Pyfun)
" Regex-based and deliberately small — diagnostics, hover, and navigation
" come from the language server (`pyfun lsp`), not from this file.

if exists('b:current_syntax')
  finish
endif

" Declarations and control flow
syn keyword pyfunDeclaration let mut pure type measure module extern import fun
syn keyword pyfunConditional if then elif else
syn keyword pyfunKeyword match case with do in return yield
syn keyword pyfunOperatorWord and or not
syn keyword pyfunBoolean true false

" Built-in computation-expression builders (async { … }, seq { … }, result { … })
syn keyword pyfunBuilder async seq result

" Comments
syn match pyfunComment "#.*$" contains=@Spell

" Type and constructor names (uppercase-initial identifiers)
syn match pyfunType "\<[A-Z][A-Za-z0-9_']*\>"

" Operators: pipes, composition, arrows, mutation
syn match pyfunOperator "|>\|<|\|>>\|<<\|->\|<-"
syn match pyfunOperator "==\|!=\|<=\|>=\|\*\*\|//"

" Numbers: ints/floats with digit separators, scientific notation,
" hex/octal/binary, and an optional unit-of-measure suffix like 9.81<m/s^2>
syn match pyfunNumber "\<\d[0-9_]*\%(\.\d[0-9_]*\)\?\%([eE][+-]\?\d\+\)\?\%(<[^<>]\+>\)\?"
syn match pyfunNumber "\<0[xX][0-9a-fA-F_]\+\>"
syn match pyfunNumber "\<0[oO][0-7_]\+\>"
syn match pyfunNumber "\<0[bB][01_]\+\>"

" Strings: plain, f-interpolated, raw, and triple-quoted variants
syn region pyfunTripleString start=+[fr]\?"""+ end=+"""+ contains=pyfunEscape
syn region pyfunRawString start=+r"+ end=+"+ oneline
syn region pyfunString start=+f\?"+ skip=+\\"+ end=+"+ oneline contains=pyfunEscape
syn match pyfunEscape contained "\\."

hi def link pyfunDeclaration Keyword
hi def link pyfunConditional Conditional
hi def link pyfunKeyword Keyword
hi def link pyfunOperatorWord Operator
hi def link pyfunBoolean Boolean
hi def link pyfunBuilder Special
hi def link pyfunComment Comment
hi def link pyfunType Type
hi def link pyfunOperator Operator
hi def link pyfunNumber Number
hi def link pyfunTripleString String
hi def link pyfunRawString String
hi def link pyfunString String
hi def link pyfunEscape SpecialChar

let b:current_syntax = 'pyfun'
