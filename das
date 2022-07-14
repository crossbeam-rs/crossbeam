// stylelint-disable

// This is copied from https://github.com/primer/codemirror-github-light/blob/master/lib/codemirror-github-light-theme.css

/*!
 * GitHub Light v0.4.2
 * Copyright (c) 2012 - 2017 GitHub, Inc.
 * Licensed under MIT (https://github.com/primer/github-syntax-theme-generator/blob/master/LICENSE)
 */

.cm-s-github-light.CodeMirror {
  background: var(--color-codemirror-bg); // #fff;
  color: var(--color-codemirror-text); // #24292e;
}

.cm-s-github-light .CodeMirror-gutters {
  background: var(--color-codemirror-gutters-bg); // #fff;
  border-right-width: 0;
}

.cm-s-github-light .CodeMirror-guttermarker {
  color: var(--color-codemirror-guttermarker-text); // white;
}

.cm-s-github-light .CodeMirror-guttermarker-subtle {
  color: var(--color-codemirror-guttermarker-subtle-text); // #d0d0d0; No match! But close to gray-3
}

.cm-s-github-light .CodeMirror-scrollbar-filler,
.cm-s-github-light .CodeMirror-gutter-filler {
  background-color: transparent;
}

.cm-s-github-light .CodeMirror-linenumber {
  color: var(--color-codemirror-linenumber-text); // #959da5;
  padding: 0 16px 0 16px;
}

.cm-s-github-light .CodeMirror-cursor {
  border-left: 1px solid var(--color-codemirror-cursor); // #24292e;
}

.cm-s-github-light.CodeMirror-focused .CodeMirror-selected,
.cm-s-github-light .CodeMirror-line::selection,
.cm-s-github-light .CodeMirror-line > span::selection,
.cm-s-github-light .CodeMirror-line > span > span::selection {
  background: var(--color-codemirror-selection-bg, #d7d4f0); // #c8c8fa
}

// Note: The ::-moz-selection selector needs to be used separatly otherwise the whole rule doesn't work.
// For that reason the color (#c8c8fa) defined in the theme wasn't working in production
// and the default selection color (#d7d4f0) from CodeMirror
// https://github.com/codemirror/CodeMirror/blob/8bc57f76383e62e1a03c7d97c9eac74493fdbedc/lib/codemirror.css#L326
// was used instead. So here we use the same #d7d4f0 to not cause any regression.

.cm-s-github-light .CodeMirror-line::-moz-selection,
.cm-s-github-light .CodeMirror-line > span::-moz-selection,
.cm-s-github-light .CodeMirror-line > span > span::-moz-selection {
  background: var(--color-codemirror-selection-bg, #d7d4f0); // #c8c8fa
}

.cm-s-github-light .CodeMirror-activeline-background {
  background: var(--color-codemirror-activeline-bg); // #fafbfc;
}

.cm-s-github-light .CodeMirror-matchingbracket {
  text-decoration: underline;
  color: var(--color-codemirror-matchingbracket-text) !important; // #24292e;
}

.cm-s-github-light .CodeMirror-lines {
  font-family: "SFMono-Regular", Consolas, "Liberation Mono", Menlo, Courier, monospace;
  font-size: 12px;
  background: var(--color-codemirror-lines-bg); // #fff;
  line-height: 1.5;
}

.cm-s-github-light .cm-comment {
  color: var(--color-codemirror-syntax-comment); // #6a737d;
}

.cm-s-github-light .cm-constant {
  color: var(--color-codemirror-syntax-constant); // #005cc5;
}

.cm-s-github-light .cm-entity {
  font-weight: normal;
  font-style: normal;
  text-decoration: none;
  color: var(--color-codemirror-syntax-entity); // #6f42c1;
}

.cm-s-github-light .cm-keyword {
  font-weight: normal;
  font-style: normal;
  text-decoration: none;
  color: var(--color-codemirror-syntax-keyword); // #d73a49;
}

.cm-s-github-light .cm-storage {
  color: var(--color-codemirror-syntax-storage); // #d73a49;
}

.cm-s-github-light .cm-string {
  font-weight: normal;
  font-style: normal;
  text-decoration: none;
  color: var(--color-codemirror-syntax-string); // #032f62;
}

.cm-s-github-light .cm-support {
  font-weight: normal;
  font-style: normal;
  text-decoration: none;
  color: var(--color-codemirror-syntax-support); // #005cc5;
}

.cm-s-github-light .cm-variable {
  font-weight: normal;
  font-style: normal;
  text-decoration: none;
  color: var(--color-codemirror-syntax-variable); // #e36209;
}
