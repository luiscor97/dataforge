# Fuzzing

Harnesses `cargo-fuzz` (libFuzzer) para los parsers que reciben datos no
confiables. Cada objetivo prueba una invariante: **el parseo nunca entra en
pánico** — o produce un valor o un error tipado.

| Objetivo | Parser | Invariante |
| --- | --- | --- |
| `fingerprint_parse` | `FileFingerprint::parse` | token de una base que un atacante pudo editar (ADR-0019) |
| `raw_path_from_blob` | `RawPath::from_blob` | blob de ruta raw almacenado (ADR-0020) |
| `extract_worker_response` | `worker_protocol::parse_response` | frame de un sidecar hostil/corrupto (ADR-0031) |
| `safe_relative_path` | `SafeRelativePath::parse` | ruta relativa no confiable (ADR-0017) |

Es un workspace independiente (necesita nightly + libFuzzer) excluido del
build stable del workspace padre.

## Ejecutar (Linux/macOS)

```bash
cargo install cargo-fuzz
cargo +nightly fuzz build              # compila todos los objetivos
cargo +nightly fuzz run fingerprint_parse -- -max_total_time=60
```

En CI, el job `Fuzz targets (experimental)` compila los cuatro y hace una
pasada corta de cada uno en ubuntu (`continue-on-error`).
