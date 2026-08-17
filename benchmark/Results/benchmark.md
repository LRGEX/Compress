# LRGEX Compress - Fair Compression Benchmark Results

Generated: 2026-08-17 04:29:01

## Methodology
- 5 datasets, 4 compressors, **100 runs total**, **0 integrity failures**
- Each application/dataset combination ran **5 times**; **median** is published
- All apps used their **fastest legitimate compression preset** (no Store/copy mode)
- LRGEX: zero configuration (as shipped)
- 7-Zip: Fastest (-mx=1) | WinRAR: Fastest (-m1 -r) | Windows ZIP: Compress-Archive Fastest
- Integrity verified: file count + size match on extraction, 100/100 PASS

Benchmark performed on Windows using LRGEX Compress, 7-Zip, WinRAR and Windows ZIP.
Five runs per configuration; median results shown. No store/no-compression mode.
All archives were extracted and verified.

## Dataset: AlreadyCompressed-1GB

| Application | Mode | Archive Size | Comp Time (s) | Speed (MB/s) | Extract (s) | Ratio | Integrity |
|---|---|---:|---:|---:|---:|---:|---|
| LRGEX | Zero configuration | 1040 MB | 4.04 | 257.2 | 3.07 | 1 | PASS |
| 7-Zip | Fastest (-mx=1) | 1040.1 MB | 8.52 | 122 | 1.08 | 1 | PASS |
| WinRAR | Fastest (-m1) | 1040 MB | 12.79 | 81.3 | 1.06 | 1 | PASS |
| WindowsZIP | Fastest (Compress-Archive) | 1040.3 MB | 32.18 | 32.3 | 1.86 | 1 | PASS |

## Dataset: HighlyCompressible-1GB

| Application | Mode | Archive Size | Comp Time (s) | Speed (MB/s) | Extract (s) | Ratio | Integrity |
|---|---|---:|---:|---:|---:|---:|---|
| LRGEX | Zero configuration | 64.9 MB | 2.79 | 100.3 | 2.69 | 4.317 | PASS |
| 7-Zip | Fastest (-mx=1) | 63.5 MB | 1.02 | 275.1 | 0.39 | 4.411 | PASS |
| WinRAR | Fastest (-m1) | 64.1 MB | 2.11 | 132.6 | 0.95 | 4.368 | PASS |
| WindowsZIP | Fastest (Compress-Archive) | 72.2 MB | 4.15 | 67.5 | 2.29 | 3.88 | PASS |

## Dataset: LargeFiles-3GB

| Application | Mode | Archive Size | Comp Time (s) | Speed (MB/s) | Extract (s) | Ratio | Integrity |
|---|---|---:|---:|---:|---:|---:|---|
| LRGEX | Zero configuration | 3672.1 MB | 7.38 | 497.4 | 5.48 | 1 | PASS |
| 7-Zip | Fastest (-mx=1) | 3672.2 MB | 27.24 | 134.8 | 3.42 | 1 | PASS |
| WinRAR | Fastest (-m1) | 3672 MB | 44.03 | 83.4 | 3.14 | 1 | PASS |
| WindowsZIP | Fastest (Compress-Archive) | 3673.1 MB | 111.36 | 33 | 3.96 | 1 | PASS |

## Dataset: ManySmallFiles-1GB

| Application | Mode | Archive Size | Comp Time (s) | Speed (MB/s) | Extract (s) | Ratio | Integrity |
|---|---|---:|---:|---:|---:|---:|---|
| LRGEX | Zero configuration | 54.3 MB | 2.79 | 19.4 | 14.34 | 0.994 | PASS |
| 7-Zip | Fastest (-mx=1) | 54.1 MB | 1.41 | 38.4 | 7.48 | 0.998 | PASS |
| WinRAR | Fastest (-m1) | 55.2 MB | 7.87 | 6.9 | 17.32 | 0.979 | PASS |
| WindowsZIP | Fastest (Compress-Archive) | 55.5 MB | 153.86 | 0.4 | 127.86 | 0.974 | PASS |

## Dataset: Mixed-3GB

| Application | Mode | Archive Size | Comp Time (s) | Speed (MB/s) | Extract (s) | Ratio | Integrity |
|---|---|---:|---:|---:|---:|---:|---|
| LRGEX | Zero configuration | 1390.5 MB | 4.46 | 313.8 | 3.68 | 1.005 | PASS |
| 7-Zip | Fastest (-mx=1) | 1391 MB | 10.75 | 130 | 1.43 | 1.005 | PASS |
| WinRAR | Fastest (-m1) | 1202.4 MB | 15.56 | 89.9 | 1.46 | 1.163 | PASS |
| WindowsZIP | Fastest (Compress-Archive) | 1392.6 MB | 44.44 | 31.5 | 3.22 | 1.004 | PASS |

## Key Findings

**LRGEX delivers the fastest compression in 3 of 5 real-world benchmark categories, with 1.7x higher overall median throughput than 7-Zip.**

| Dataset | LRGEX vs 7-Zip |
|---|---|
| Large files | 3.69x faster |
| Mixed content | 2.42x faster |
| Already compressed | 2.11x faster |
| Highly compressible | 2.75x slower |
| Many small files | 1.98x slower |

LRGEX is built for speed. It is not universally the fastest — 7-Zip has advantages on highly compressible data and huge numbers of tiny files. The compression ratio trade-off is minimal on compressible data (4.317 vs 4.411), and WinRAR achieves better density on mixed content (1.163 vs 1.005) at the cost of being ~3.5x slower.

### Honest limitations
- 7-Zip extracts ~2x faster than LRGEX across most datasets
- WinRAR produces smaller archives on mixed content
- Many-small-files workloads favor 7-Zip (LRGEX per-file overhead adds up)

## Speed Ranking (median compression throughput across all datasets)

LRGEX: **235.6 MB/s**

7-Zip: **138 MB/s**

WinRAR: **78.9 MB/s**

WindowsZIP: **32.6 MB/s**


