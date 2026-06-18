---
title: Field Notes on Pomes
author: Jane Quince
date: 2026-06-18
keywords: [apple, pear, y]
draft: false
revision: 007
ratio: 1.5
abstract: |
  A short standing note on the *pome* fruits, gathered
  over one season.

  It runs to a couple of short paragraphs.
contributors:
  - name: A. Medlar
    role: editor
  - name: B. Sorbus
    role: reviewer
notes: ~
---

The block fenced by `---` above is lifted into the document's metadata and removed
from the body, so this paragraph is the first thing that remains.

A bare `y` in the keyword list resolves to a boolean rather than a string, the
revision `007` is canonicalized to a plain integer, and `false` becomes a real
boolean. The abstract, written as a literal block scalar, keeps its paragraph
break and so becomes block-level content. A `~` stands for an empty value.
