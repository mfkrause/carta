Inline `raw text` and #raw("call form") in a paragraph.

```
An unlabelled raw block
spanning two lines.
```

```rust
fn main() {
    println!("hi");
}
```

#raw(block: true, lang: "python", "print(1)")

```{r}
plot(1)
```

```rust,ignore
fn main() {}
```

```python print(1)```

#box(```rust
fn nested() {}
```)

#list(`inline raw`, [plain item])
