# Syntax grammar pack (copyleft and unspecified licenses)

The grammar files in this directory are third-party syntax definitions in the
KDE syntax-highlighting XML format. Unlike the definitions in `../syntax/`,
they are not covered by carta's MIT OR Apache-2.0 license: each file
carries its own upstream license, declared in the `license` attribute of its
`<language>` element and/or its header comment. The set includes LGPL-, GPL-,
and GFDL-licensed files, as well as files whose headers declare no license.

Because of that, these grammars are not compiled into carta by default. They
are loaded at runtime instead:

- The prebuilt release archives ship this directory as `syntax/` next to the
  `carta` binary, where it is discovered automatically.
- A different directory can be selected with the `CARTA_SYNTAX_DIR`
  environment variable (set it to an empty string to disable directory
  loading), and single files can be added with `--syntax-definition`.
- Building with the `embed-copyleft-grammars` cargo feature embeds them like
  the permissive set. A binary built this way includes copyleft-licensed data;
  distributing it means complying with the licenses of every file here.

Redistribute this directory only together with the license information the
files carry. Each XML file is its own preferred form for modification, so
shipping the files unmodified satisfies the source-availability terms of the
copyleft licenses involved.
