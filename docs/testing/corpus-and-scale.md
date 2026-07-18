# Corpus sintético y prueba de escala

Cubre los criterios de aceptación 1 («100.000 archivos») y 10 («corpus
base») del Milestone 0.1 (RFC-0001 §45) y el corpus de regresión del §40.

## El generador: `tools/df-corpus`

Genera un árbol de carpetas **determinista**: la misma semilla y la misma
especificación producen las mismas rutas relativas normalizadas y los mismos
bytes de archivo. La regresión compara ruta, tipo de entrada y contenido
completo, no solo recuentos o tamaños. No se incluyen timestamps, ACL u otros
metadatos del filesystem en el contrato determinista. Un xorshift64* propio
deriva estructura, nombres, tamaños y contenidos.

El corpus imita una colección heredada real:

- carpetas anidadas con nombres realistas (casos, clientes, año…);
- **duplicados exactos** repartidos entre ramas (porcentaje configurable);
- nombres Unicode (acentos, ñ, escritura no latina) que ejercitan el
  manejo de rutas del escáner (§13.1);
- carpetas vacías;
- una fracción de archivos que casan con las reglas declarativas (`*.tmp`);
- tamaños variados, con un archivo de ~1 MiB cada N.

Generar un corpus a mano:

```powershell
cargo run -p df-corpus --release -- `
  --output fixtures/generated-large/corpus-100k `
  --files 100000 --seed 42 --duplicate-percent 20
```

`fixtures/generated-large/` está en `.gitignore`: un corpus es regenerable
desde su semilla y **nunca se versiona**.

El destino debe no existir o estar completamente vacío. Si contiene una sola
entrada, el generador falla antes de crear el corpus. Cada archivo se abre con
semántica `create_new`: si otra aplicación planta el mismo nombre después de
la comprobación, la generación falla y conserva sus bytes en vez de truncarlo.
Un fallo posterior puede dejar un corpus parcial; DataForge no borra
implícitamente ese directorio y el reintento debe usar otro destino vacío.

## Las pruebas

`tools/df-corpus/tests/scale_pipeline.rs` conduce el corpus por el motor
completo vía `df-facade` — crear → escanear → hashear → analizar → planificar
→ aprobar → ejecutar → verificar — y afirma las invariantes que dan sentido
a la ejecución:

- el escaneo ve exactamente los archivos generados, sin errores;
- todo se hashea (sin fallos, sin pendientes);
- el análisis completa y produce conjuntos de duplicados;
- la ejecución termina sin operaciones fallidas ni pendientes;
- la verificación devuelve `COMPLETED`;
- **el origen queda intacto** (regla 1), comprobado antes y después mediante
  rutas relativas, tipo de cada entrada y SHA-256 de cada archivo;
- el ledger verifica criptográficamente al final.

Dos sabores del mismo recorrido:

1. **CI (rápida)** — `a_small_corpus_survives_the_full_pipeline`: 300
   archivos, corre en cada push con `cargo test`.
2. **Escala (bajo demanda)** — `scale_full_pipeline`, marcada `#[ignore]`:

```powershell
# El criterio de aceptación de M0.1 (100.000 archivos por defecto):
cargo test -p df-corpus --release -- --ignored scale --nocapture

# Otra escala:
$env:DF_CORPUS_FILES = "10000"
cargo test -p df-corpus --release -- --ignored scale --nocapture
```

La prueba imprime la duración de cada fase (`[scan]`, `[hash]`, …) para
detectar regresiones de rendimiento entre versiones.

## Qué NO es

- No es un benchmark formal (§41): no fija umbrales de tiempo, solo
  invariantes de corrección. Los benchmarks llegan con su propio hito.
- No sustituye a los tests adversariales (junctions, TOCTOU, manipulación),
  que viven junto a cada crate endurecido.
