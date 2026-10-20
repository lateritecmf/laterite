// The rich-text engine compiled into laterite-admin: TipTap 3 over ProseMirror.
// Everything the `richeditor` field's island needs is exported from here under
// one global, so the island itself stays a plain script in assets/fields.
import 'prosemirror-view/style/prosemirror.css';

export { Editor, Extension, Node, Mark, mergeAttributes } from '@tiptap/core';
export { default as StarterKit } from '@tiptap/starter-kit';
export { default as Image } from '@tiptap/extension-image';
export { default as Placeholder } from '@tiptap/extension-placeholder';
export { Table, TableRow, TableCell, TableHeader } from '@tiptap/extension-table';
export { default as Suggestion } from '@tiptap/suggestion';
export { Plugin, PluginKey, TextSelection, NodeSelection } from '@tiptap/pm/state';
export { Decoration, DecorationSet } from '@tiptap/pm/view';
export { DOMParser, DOMSerializer, Fragment, Slice } from '@tiptap/pm/model';
