// Registers a `pyfun` language with the highlight.js bundled by mdBook, then re-runs
// highlighting on any ```pyfun code blocks (mdBook highlights the page before
// additional-js scripts load, so those blocks were processed before the language
// existed). Token rules mirror editors/vscode/pyfun.tmLanguage.json.
(function () {
  "use strict";
  if (typeof hljs === "undefined") {
    return;
  }

  function pyfun(hljs) {
    return {
      name: "Pyfun",
      keywords: {
        // `mut` and `extern` are deliberately absent: they match as pyfun-escape
        // modes below so CSS can tint the escape hatches amber, as the editors do.
        keyword:
          "let pure fun type measure module import match case with if then elif else " +
          "and or not try do in return yield async seq result",
        literal: "true false",
        built_in: "print",
      },
      contains: [
        hljs.COMMENT(/#/, /$/),
        // Triple-quoted strings first so a lone `"` rule cannot eat their opener.
        { className: "string", begin: /[fr]?"""/, end: /"""/ },
        {
          className: "string",
          begin: /[fr]?"/,
          end: /"/,
          illegal: /\n/,
          contains: [{ begin: /\\./ }],
        },
        // The escape hatches: mutation and the FFI boundary.
        { className: "pyfun-escape", begin: /\b(?:mut|extern)\b/ },
        { className: "pyfun-escape", begin: /<-/ },
        // Typed holes: `?` or `?name`.
        { className: "pyfun-hole", begin: /\?[a-z_][A-Za-z0-9_']*|\?(?![a-zA-Z_?])/ },
        // Hex/octal/binary before decimal, so `0x` is not read as `0` then `x`.
        { className: "number", begin: /\b0[xX][0-9a-fA-F_]+|\b0[oO][0-7_]+|\b0[bB][01_]+/ },
        // Decimal int/float with optional exponent and optional adjacent unit-of-measure
        // annotation (5<m>, 3.0<m/s^2>) — the unit must touch the digits, as in the lexer.
        {
          className: "number",
          begin: /\b\d[\d_]*(?:\.\d[\d_]*)?(?:[eE][+-]?\d+)?(?:<[^<>\n]+>)?/,
        },
        // Type names, constructors, and module namespaces: upper-case-led identifiers.
        { className: "type", begin: /\b[A-Z][A-Za-z0-9_']*/ },
      ],
    };
  }

  hljs.registerLanguage("pyfun", pyfun);

  var highlight = hljs.highlightElement || hljs.highlightBlock;
  document.querySelectorAll("code.language-pyfun").forEach(function (block) {
    // highlight.js v11 refuses to touch a block it already visited; clear the marker.
    if (block.dataset) {
      delete block.dataset.highlighted;
    }
    highlight.call(hljs, block);
  });
})();
