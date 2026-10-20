// The code engine compiled into laterite-admin: CodeMirror 6, with the
// languages the admin edits (templates, styles, scripts, data, prose).
export { EditorView, keymap, lineNumbers, highlightActiveLine, highlightActiveLineGutter, drawSelection, dropCursor, rectangularSelection, crosshairCursor, highlightSpecialChars, placeholder, Decoration, WidgetType, ViewPlugin } from '@codemirror/view';
export { EditorState, Compartment, StateField, StateEffect, EditorSelection, Transaction } from '@codemirror/state';
export { defaultKeymap, history, historyKeymap, indentWithTab, undo, redo } from '@codemirror/commands';
export { syntaxHighlighting, defaultHighlightStyle, indentOnInput, bracketMatching, foldGutter, foldKeymap, indentUnit, StreamLanguage, LanguageDescription } from '@codemirror/language';
export { searchKeymap, highlightSelectionMatches, openSearchPanel } from '@codemirror/search';
export { html } from '@codemirror/lang-html';
export { css } from '@codemirror/lang-css';
export { javascript } from '@codemirror/lang-javascript';
export { json } from '@codemirror/lang-json';
export { yaml } from '@codemirror/lang-yaml';
export { markdown, markdownLanguage, markdownKeymap } from '@codemirror/lang-markdown';
export { jinja } from '@codemirror/lang-jinja';
export { HighlightStyle } from '@codemirror/language';
export { tags } from '@lezer/highlight';
