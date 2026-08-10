#let stock = ("pen", "ink", "paper")

#if "ink" in stock [Ink is stocked.] else [Ink is missing.]

#if "chalk" not in stock [Chalk is missing.] else [Chalk is stocked.]

#let notion = 5
The notion is #notion.

#if "a" not in "banana" [No a.] else [Has an a.]

Nested: #if not ("pen" not in stock) [pen is there].
