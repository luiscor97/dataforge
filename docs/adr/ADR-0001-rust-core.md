# ADR-0001 — Rust como núcleo del motor

**Estado:** Aceptada
**Fecha:** 2026-07-13
**Relacionada con:** RFC-0001 §6 (ADR-0001), §5.5 (engine-first)

## Contexto

DataForge procesa cientos de miles de archivos con garantías de no
destrucción, verificación criptográfica y reanudación. Necesita control fino
de E/S, concurrencia segura, ejecutables autocontenidos y una integración
natural con Tauri 2 y con el ecosistema de análisis (Arrow, DataFusion,
Tantivy, Wasmtime).

## Decisión

El motor (`crates/df-*`) se implementa íntegramente en Rust. La aplicación de
escritorio es un cliente del motor a través de `df-facade`; la CLI es otro
cliente del mismo crate. Ningún cliente contiene lógica crítica.

Python queda permitido solo para prototipos, notebooks de investigación,
generación de fixtures y comparación de algoritmos; nunca como dependencia
del producto.

## Alternativas consideradas

- **Python + empaquetado**: velocidad de desarrollo inicial mayor, pero
  distribución frágil, rendimiento insuficiente para millones de entradas y
  sin garantías de memoria.
- **C++/Qt**: rendimiento equivalente, pero sin seguridad de memoria y con un
  coste de contribución mucho mayor para un proyecto open source.
- **Node/Electron**: descartado por consumo de memoria y por mover lógica
  crítica al runtime de UI, contra la regla 16 del RFC.

## Consecuencias

- Compilaciones más lentas y curva de entrada Rust para contribuyentes.
- Un único lenguaje para dominio, persistencia, ledger y ejecución segura.
- El workspace Cargo es el contrato de compilación del repositorio: cada hito
  debe terminar con `cargo fmt --check`, `cargo clippy` y `cargo test` verdes.
