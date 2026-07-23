# Benchmarks — carta vs pandoc

- **Reference machine:** Apple M1 Pro (10 cores), 16 GB RAM, macOS 26.5 (arm64)
- **carta:** 0.0.7
- **pandoc:** 3.10
- **Driver:** hyperfine 1.20.0, warmup 3, 12 runs

## Headline

- **~8–22× faster** end-to-end across formats and sizes; up to **~46×** on individual reader/writer surfaces.
- **~20× smaller** binary (9.1 MB vs 179.8 MB).
- **~4–25× less peak memory**.

## How to read this

Both tools run with identical `-f/-t` flags; pandoc is configured so both tools produce equivalent output and do equivalent work. Times are wall-clock end-to-end (process start included). `speedup` = pandoc mean ÷ carta mean. `MB/s` is carta throughput over the actual input size. RSS is peak resident memory from a single `/usr/bin/time` run. The HTML and LaTeX targets include syntax highlighting of code blocks in both tools.

## reader — commonmark → json

| size   | carta mean ± σ      | pandoc mean ± σ     | speedup | carta MB/s | carta RSS | pandoc RSS |
|--------|---------------------|---------------------|---------|------------|-----------|------------|
| 13 KB  |     3.94 ms ± 0.23  |    44.13 ms ± 0.83  |   11.2x |        3.2 |    4.5 MB |   107.8 MB |
| 101 KB |     5.45 ms ± 0.70  |    93.33 ms ± 1.61  |   17.1x |       18.1 |    8.1 MB |   121.8 MB |
| 1 MB   |    24.48 ms ± 0.38  |   539.33 ms ± 11.21 |   22.0x |       40.9 |   49.0 MB |   225.8 MB |

## reader — html → json

| size   | carta mean ± σ      | pandoc mean ± σ     | speedup | carta MB/s | carta RSS | pandoc RSS |
|--------|---------------------|---------------------|---------|------------|-----------|------------|
| 14 KB  |     3.02 ms ± 0.08  |    55.49 ms ± 1.19  |   18.4x |        4.7 |    4.3 MB |   108.7 MB |
| 112 KB |     5.71 ms ± 0.51  |   176.91 ms ± 6.48  |   31.0x |       19.1 |    8.1 MB |   133.8 MB |
| 1 MB   |    27.03 ms ± 3.18  |  1240.55 ms ± 53.35 |   45.9x |       40.8 |   51.3 MB |   428.8 MB |

## writer — json → html

| size   | carta mean ± σ      | pandoc mean ± σ     | speedup | carta MB/s | carta RSS | pandoc RSS |
|--------|---------------------|---------------------|---------|------------|-----------|------------|
| 12 KB  |     5.96 ms ± 0.11  |    30.52 ms ± 0.40  |    5.1x |        2.0 |    7.2 MB |    43.7 MB |
| 113 KB |     6.10 ms ± 0.05  |    42.13 ms ± 3.75  |    6.9x |       18.0 |    7.5 MB |    62.2 MB |
| 1 MB   |     9.88 ms ± 2.37  |   113.38 ms ± 5.11  |   11.5x |      112.9 |    9.9 MB |   123.3 MB |

## writer — json → latex

| size   | carta mean ± σ      | pandoc mean ± σ     | speedup | carta MB/s | carta RSS | pandoc RSS |
|--------|---------------------|---------------------|---------|------------|-----------|------------|
| 12 KB  |     5.92 ms ± 0.28  |    30.46 ms ± 0.20  |    5.1x |        2.0 |    7.3 MB |    41.8 MB |
| 113 KB |     6.25 ms ± 0.20  |    30.67 ms ± 0.24  |    4.9x |       17.6 |    7.7 MB |    56.6 MB |
| 1 MB   |     9.73 ms ± 0.09  |   107.23 ms ± 4.29  |   11.0x |      114.6 |   10.2 MB |   122.8 MB |

## writer — json → rst

| size   | carta mean ± σ      | pandoc mean ± σ     | speedup | carta MB/s | carta RSS | pandoc RSS |
|--------|---------------------|---------------------|---------|------------|-----------|------------|
| 12 KB  |     2.79 ms ± 0.08  |    32.77 ms ± 3.93  |   11.7x |        4.3 |    3.5 MB |    39.5 MB |
| 113 KB |     3.54 ms ± 0.25  |    30.89 ms ± 1.24  |    8.7x |       31.1 |    4.0 MB |    41.2 MB |
| 1 MB   |     7.24 ms ± 0.81  |    94.98 ms ± 1.37  |   13.1x |      154.0 |    6.5 MB |   122.3 MB |

## writer — json → plain

| size   | carta mean ± σ      | pandoc mean ± σ     | speedup | carta MB/s | carta RSS | pandoc RSS |
|--------|---------------------|---------------------|---------|------------|-----------|------------|
| 12 KB  |     3.68 ms ± 0.89  |    31.87 ms ± 0.81  |    8.7x |        3.3 |    3.4 MB |    39.8 MB |
| 113 KB |     3.71 ms ± 0.28  |    31.51 ms ± 0.91  |    8.5x |       29.6 |    4.0 MB |    62.5 MB |
| 1 MB   |     5.75 ms ± 0.08  |    98.10 ms ± 5.78  |   17.1x |      194.2 |    6.5 MB |   122.7 MB |

## writer — json → commonmark

| size   | carta mean ± σ      | pandoc mean ± σ     | speedup | carta MB/s | carta RSS | pandoc RSS |
|--------|---------------------|---------------------|---------|------------|-----------|------------|
| 12 KB  |     3.16 ms ± 0.41  |    32.19 ms ± 1.23  |   10.2x |        3.8 |    3.5 MB |    41.8 MB |
| 113 KB |     3.77 ms ± 0.28  |    46.24 ms ± 4.55  |   12.3x |       29.2 |    4.1 MB |    63.3 MB |
| 1 MB   |     6.70 ms ± 0.58  |   205.88 ms ± 20.06 |   30.7x |      166.6 |    6.8 MB |   123.6 MB |

## writer — json → mediawiki

| size   | carta mean ± σ      | pandoc mean ± σ     | speedup | carta MB/s | carta RSS | pandoc RSS |
|--------|---------------------|---------------------|---------|------------|-----------|------------|
| 12 KB  |     3.29 ms ± 0.26  |    31.54 ms ± 0.97  |    9.6x |        3.7 |    3.5 MB |    39.5 MB |
| 113 KB |     3.40 ms ± 0.14  |    31.09 ms ± 0.91  |    9.1x |       32.3 |    4.0 MB |    43.0 MB |
| 1 MB   |     5.57 ms ± 0.07  |    95.05 ms ± 4.07  |   17.1x |      200.2 |    6.5 MB |   122.2 MB |

## writer — json → native

| size   | carta mean ± σ      | pandoc mean ± σ     | speedup | carta MB/s | carta RSS | pandoc RSS |
|--------|---------------------|---------------------|---------|------------|-----------|------------|
| 12 KB  |     2.74 ms ± 0.14  |    31.29 ms ± 0.54  |   11.4x |        4.4 |    3.3 MB |    39.0 MB |
| 113 KB |     3.58 ms ± 0.20  |    43.67 ms ± 1.07  |   12.2x |       30.8 |    4.1 MB |    42.8 MB |
| 1 MB   |     7.83 ms ± 0.67  |   182.55 ms ± 6.65  |   23.3x |      142.4 |    9.0 MB |   143.1 MB |

## writer — json → json

| size   | carta mean ± σ      | pandoc mean ± σ     | speedup | carta MB/s | carta RSS | pandoc RSS |
|--------|---------------------|---------------------|---------|------------|-----------|------------|
| 12 KB  |     3.20 ms ± 0.25  |    32.14 ms ± 1.21  |   10.0x |        3.8 |    3.2 MB |    38.9 MB |
| 113 KB |     3.61 ms ± 0.19  |    31.16 ms ± 0.97  |    8.6x |       30.5 |    3.6 MB |    40.0 MB |
| 1 MB   |     4.86 ms ± 0.10  |    83.62 ms ± 3.96  |   17.2x |      229.7 |    6.1 MB |   121.9 MB |

## e2e — commonmark → html

| size   | carta mean ± σ      | pandoc mean ± σ     | speedup | carta MB/s | carta RSS | pandoc RSS |
|--------|---------------------|---------------------|---------|------------|-----------|------------|
| 13 KB  |     6.98 ms ± 0.38  |    57.75 ms ± 1.25  |    8.3x |        1.8 |    8.2 MB |   109.1 MB |
| 101 KB |    11.20 ms ± 0.30  |   159.03 ms ± 4.27  |   14.2x |        8.8 |   11.2 MB |   122.2 MB |
| 1 MB   |    55.17 ms ± 1.47  |  1187.32 ms ± 25.84 |   21.5x |       18.1 |   45.8 MB |   246.2 MB |

## e2e — commonmark → latex

| size   | carta mean ± σ      | pandoc mean ± σ     | speedup | carta MB/s | carta RSS | pandoc RSS |
|--------|---------------------|---------------------|---------|------------|-----------|------------|
| 13 KB  |     6.89 ms ± 0.28  |    55.58 ms ± 1.69  |    8.1x |        1.8 |    8.3 MB |   107.6 MB |
| 101 KB |    11.74 ms ± 0.38  |   137.08 ms ± 4.92  |   11.7x |        8.4 |   11.3 MB |   123.6 MB |
| 1 MB   |    62.75 ms ± 1.74  |  1102.77 ms ± 34.75 |   17.6x |       15.9 |   47.2 MB |   254.7 MB |

## e2e — commonmark → rst

| size   | carta mean ± σ      | pandoc mean ± σ     | speedup | carta MB/s | carta RSS | pandoc RSS |
|--------|---------------------|---------------------|---------|------------|-----------|------------|
| 13 KB  |     3.36 ms ± 0.06  |    44.11 ms ± 3.78  |   13.1x |        3.8 |    5.0 MB |   106.9 MB |
| 101 KB |     6.77 ms ± 0.13  |   118.74 ms ± 4.60  |   17.5x |       14.6 |    8.3 MB |   122.0 MB |
| 1 MB   |    43.18 ms ± 0.93  |   819.69 ms ± 27.61 |   19.0x |       23.2 |   43.1 MB |   237.0 MB |

## e2e — commonmark → json

| size   | carta mean ± σ      | pandoc mean ± σ     | speedup | carta MB/s | carta RSS | pandoc RSS |
|--------|---------------------|---------------------|---------|------------|-----------|------------|
| 13 KB  |     3.14 ms ± 0.12  |    44.01 ms ± 0.56  |   14.0x |        4.1 |    4.5 MB |   107.8 MB |
| 101 KB |     5.13 ms ± 0.09  |    92.91 ms ± 2.23  |   18.1x |       19.3 |    8.1 MB |   121.8 MB |
| 1 MB   |    26.09 ms ± 0.94  |   529.71 ms ± 5.12  |   20.3x |       38.4 |   48.9 MB |   225.8 MB |

## startup — commonmark → html (near-empty input)

| size   | carta mean ± σ      | pandoc mean ± σ     | speedup | carta MB/s | carta RSS | pandoc RSS |
|--------|---------------------|---------------------|---------|------------|-----------|------------|
| 27 B   |     2.63 ms ± 0.16  |    30.70 ms ± 0.62  |   11.7x |        0.0 |    3.3 MB |    41.0 MB |

## startup — commonmark → json (near-empty input)

| size   | carta mean ± σ      | pandoc mean ± σ     | speedup | carta MB/s | carta RSS | pandoc RSS |
|--------|---------------------|---------------------|---------|------------|-----------|------------|
| 27 B   |     2.67 ms ± 0.23  |    31.17 ms ± 1.21  |   11.7x |        0.0 |    3.4 MB |    39.4 MB |

## binary size

| binary | size       | ratio |
|--------|------------|-------|
| carta  |     9.1 MB |  1.0x |
| pandoc |   179.8 MB |   20x |
