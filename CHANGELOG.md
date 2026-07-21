# Changelog

Formato: [Keep a Changelog](https://keepachangelog.com/es/1.1.0/).
Versionado: [SemVer](https://semver.org/lang/es/).

## [Unreleased]

### M1.0.1 — Performance Engineering (en curso)

#### Añadido

- Benchmark reproducible: perfiles de corpus deterministas A–D en `df-corpus`
  (bandas de tamaño log-uniformes con aritmética entera + escritura en
  streaming), driver `scripts/bench/run-pipeline-bench.ps1` (mide por fase,
  CPU/memoria, throughput; JSON por caso en `docs/performance/data/`) y
  metodología (`docs/performance/benchmark-methodology.md`). Baseline en
  `docs/performance/m1.0.1-baseline.md`.
- Instrumentación por etapa del executor (`ExecuteOutcome.stage_nanos`, vía
  `--json`): mide las 12 etapas del protocolo §27.1. El desglose destapa el
  cuello de botella **medido**: copiar bytes es solo el 5,7 % del tiempo de
  ejecución; dominan los commits SQLite por operación (~32 %) y la latencia de
  syscalls por archivo (~32 %). No es ancho de banda, es latencia por archivo.
- Hashing y verificación paralelos acotados (ADR-0040): un coordinador SQLite
  único entrega trabajos inmutables a un pool acotado (`std::thread::scope`,
  sin dependencias nuevas; buffer por worker; work-stealing por índice
  atómico), `--workers auto|N`. Determinismo probado: `workers=1` y
  `workers=N` dan salida byte-idéntica. Ganancia medida ~2,5× en archivos
  grandes (techo del NVMe), ~1,26× en pequeños (latency-bound); cifras y causa
  en `docs/performance/m1.0.1-results.md`, sin maquillar.

#### Cambiado

- Los buffers de lectura del executor y del verificador se reutilizan por run
  en vez de reservarse por archivo (higiene de asignación; efecto en tiempo de
  pared dentro del ruido en estos corpus).

#### Diseño (propuesto, no implementado)

- Ejecución estricta paralela (`docs/performance/strict-parallel-execution-design.md`):
  coordinador SQLite único, exclusión por destino, protocolo §27.1 intacto y
  las seis ventanas de caída con su recuperación. A revisar antes de
  refactorizar el executor; el modo estricto actual no cambia.

## [1.0.0] — 2026-07-21 — Milestone 1.0 "Stable Reconstruction Platform"

Primera versión estable. El pipeline completo — inventario inmutable,
análisis estructural y de contenido, plan aprobado, copia verificada y
auditoría encadenada — está probado de extremo a extremo en Windows hasta
1.000.000 de archivos con verificación independiente `COMPLETED`. Los
contratos públicos (schemas, algoritmos, ABI de plugins, 19 migraciones)
quedan congelados bajo test de regresión (ADR-0037); congelar es subir
versión + ADR, nunca editar in place.

#### Cambiado

- Versión del workspace, CLI y escritorio: `0.2.0` → `1.0.0`.
- `EXTRACTOR_VERSION` desacoplado de la versión del crate y congelado como
  literal `0.2.0+content-v1`: es una identidad de algoritmo sellada en la
  evidencia existente, no una versión de software — derivarla del crate
  habría re-clavado silenciosamente cada representación almacenada en un
  bump sin cambio semántico. El `0.2.0` inicial queda como token histórico
  (ADR-0037).

#### Alcance declarado de la 1.0 (con veto ejercitable antes del tag)

- **Windows-first**: garantías de escritura probadas de extremo a extremo
  en Windows; en POSIX la ejecución se bloquea en vez de fingir y la CI de
  Linux es experimental.
- **Deuda declarada post-1.0** (ninguna toca garantías de reconstrucción):
  daemon en segundo plano (M0.8); escritura segura POSIX madura;
  independencia de ruta/máquina en builds (`--remap-path-prefix`, entorno
  canónico); firma Authenticode/SmartScreen adicional a Sigstore; pase de
  lector de pantalla real + axe en CI; revocación de plugins y
  `SubjectText` (M0.6); informes finales exportables.
- El mapa garantía→evidencia completo vive en
  `docs/release/m1.0-acceptance.md`.

### Milestone 0.9 — Stabilization

#### Añadido

- Contratos congelados (ADR-0037): el test `df-facade::frozen_contracts`
  fija en un único lugar toda versión de schema, algoritmo y ABI, más el
  número y orden de las 19 migraciones, y falla si algo cambia. Congelar
  es subir versión + ADR, nunca editar in place. Se exponen como públicos
  los identificadores de contrato `PROFILE_SCHEMA`/`PROFILE_SCHEMA_VERSION`
  y `REQUEST_SCHEMA_VERSION`.
- Compatibilidad de migraciones: tests que fijan que una instalación limpia
  aplica y verifica las 19 migraciones, que una base de una build anterior
  (a la que le falta una migración) se actualiza al abrir, y que un checksum
  manipulado (deriva silenciosa de esquema) se rechaza al abrir.
- Threat model final consolidado (`docs/threat-model/initial.md`): amplía la
  tabla de amenazas y las propiedades de fallo cerrado a M0.5–M0.8 (medio
  hostil aislado, plugin WASM sin WASI con registro firmado re-verificado,
  IA con consentimiento por digest y claves fuera de la base, reuso
  incremental solo con identidad física probada, destino degradado con
  reconocimiento explícito).
- Manual de usuario (`docs/manual/README.md`): guía completa de la CLI —
  instalación, el flujo create→scan→hash→analyze→plan→approve→execute→verify,
  informes, revisión, y las capacidades M0.3–M0.7 (similitud, contenido,
  media, plugins, IA/BYOK), perfiles, incremental/NAS, códigos de salida y
  las garantías de diseño. Enlazado desde el README.
- SBOM CycloneDX 1.5 (`docs/sbom/dataforge.cdx.json`) generado por un script
  determinista y reproducible (`scripts/generate-sbom.py`, solo cargo +
  Python 3): 786 componentes (25 crates del workspace + transitivas ancladas
  por `Cargo.lock`) con versión, licencia SPDX y PURL. Complementa a
  `cargo audit`/`cargo deny`: el SBOM enumera, las auditorías juzgan
  (`docs/sbom/README.md`). La firma queda como paso de release.
- Accesibilidad del escritorio (`docs/accessibility.md`): postura documentada
  y probada — `lang`, landmarks, jerarquía de encabezados, campos con
  `<label>`, errores en `role="alert"`, diagnósticos con `role="status"`/
  `aria-live`, y `<main aria-busy>` durante operaciones asíncronas. Pase con
  lector de pantalla real y axe en CI quedan como refuerzo de release.
- Fuzzing de los parsers de entrada no confiable (`fuzz/`, cargo-fuzz +
  libFuzzer): cuatro dianas que fijan la invariante «parsear nunca entra en
  pánico» sobre el token de fingerprint (ADR-0019), el blob de ruta raw
  (ADR-0020), el frame del worker de extracción (ADR-0031) y la ruta relativa
  segura (ADR-0017). Es un workspace propio (nightly + libFuzzer) fuera del
  build stable; el job de CI `Fuzz targets (experimental M0.9)` los compila y
  hace una pasada corta de cada uno en ubuntu (`continue-on-error`).
- Builds reproducibles (ADR-0038, `docs/release/reproducible-builds.md`):
  un doble build limpio destapó que los binarios no eran byte-idénticos —
  causa raíz verificada en la cabecera COFF: el linker rellena el
  `TimeDateStamp` PE con la hora real. Fix: `/Brepro` vía
  `.cargo/config.toml`, acotado a la toolchain MSVC de CI/release porque
  los PE con timestamp a cero disparan heurísticas de Windows Defender en
  máquinas de desarrollo (build scripts en cuarentena, os error 225 —
  observado y documentado). Los límites (independencia de ruta/máquina
  requiere `--remap-path-prefix` y entorno canónico) quedan documentados,
  no prometidos.
- Workflow de release (`.github/workflows/release.yml`): al empujar un tag
  `v*` compila los cuatro binarios con `--locked`, **prueba la
  reproducibilidad con un doble build limpio que bloquea la release si los
  hashes difieren**, publica checksums SHA-256, re-genera el SBOM y falla
  si difiere del versionado, y crea una release **en borrador** — publicar
  sigue siendo un acto humano deliberado. Ensayable sin tag vía
  `workflow_dispatch`.
- Firma de release keyless (ADR-0039): el job de release firma checksums y
  SBOM con Sigstore/cosign — certificado efímero ligado al repositorio, al
  workflow y al tag, sin claves privadas que custodiar, con las
  instrucciones de verificación en la propia release. El acto humano que la
  autoriza es empujar el tag; sustituible por certificado OV/EV antes del
  primer tag sin deuda. Con esto, las tres decisiones de scope de la 1.0
  (Windows-first, daemon post-1.0, vía de firma) quedan aplicadas y
  documentadas con veto abierto en `docs/release/m1.0-acceptance.md`.

### Milestone 0.8 — Cross-platform and Scale (cerrado Windows-first; daemon y POSIX maduro → post-1.0)

#### Añadido

- NAS endurecido (ADR-0036): clasificación real del filesystem en la
  validación (`UNC`/`DRIVE_REMOTE` → `NETWORK`; nombre de volumen para
  NTFS/ReFS/FAT32/exFAT), persistida por root y visible en el estado. El
  executor rechaza destinos sin identidad física (red, FAT, desconocidos)
  salvo reconocimiento explícito `--allow-degraded-destination` por
  ejecución; los orígenes degradados no se bloquean — sus garantías por
  archivo ya se degradan de forma visible (ADR-0019/0035).

- Snapshots incrementales (ADR-0035, migración 0019): los estados
  completados pasan a ser puntos de control reabribles hacia un nuevo
  escaneo (un plan en vuelo sigue bloqueando el rescan), y `hash
  --incremental` transporta bindings de contenido del snapshot anterior
  solo cuando el fingerprint v2 es byte-idéntico con todos los campos
  presentes; v1 o campos `none` van siempre al hash completo. Cada binding
  reusado registra su snapshot de procedencia y el evento `HASH_COMPLETED`
  cuenta `reused_from_previous_snapshot`. Modo completo por defecto
  (§14.4).

- Evidencia de 1M+ entradas (`docs/testing/m0.8-scale-1m.md`): pipeline
  completo sobre 1.000.000 de archivos (4,26 GB) — escaneo y hash sin un
  solo fallo, 160.147 conjuntos duplicados, y **1.093.705 operaciones
  ejecutadas al primer intento con 0 reintentos y verificación
  independiente `COMPLETED`**. Memoria acotada (~250 MB) durante todo el
  run. La re-huella final del origen quedó interrumpida por un reinicio de
  la máquina y se sustituyó por una comprobación post-hoc exacta de
  recuento, carpetas y bytes, con el límite documentado.
- Job de CI `Rust on Linux (experimental M0.8)` en ubuntu con
  `continue-on-error` — **en verde**: el workspace completo compila, pasa
  clippy estricto y la suite neutral de plataforma en Linux. La iteración
  fijó 11 fronteras Windows-first como comportamiento probado: gating de
  compilación en workers y suites, rechazo fail-closed de ejecución y SQL
  con lease, y reuso incremental como no-op sin identidad física.

#### Pendiente del hito

- Cache y daemon experimental; garantías de escritura segura reales en
  Linux/macOS.

### Milestone 0.7 — Assisted Intelligence (implementación local)

#### Añadido

- BYOK: las API keys de Anthropic y OpenAI viven en el almacén de
  credenciales del sistema operativo (Windows Credential Manager vía
  `keyring`), nunca en SQLite, archivos, ledger ni logs; la CLI las lee por
  stdin (`ai key set|remove|list`). OAuth no existe para terceros en estos
  proveedores y las suscripciones de chat no dan acceso a API (ADR-0034).
- Transportes cloud en el borde: `df-ai` sigue sin enlazar red ni ver
  credenciales; la fachada implementa su `CloudTransport` con `ureq`
  (rustls) para la Messages API de Anthropic y Chat Completions de OpenAI,
  extrayendo solo el texto del modelo y sin reflejar cuerpos de error.
- Consentimiento por digest: `ai explain --item <id>` sin
  `--accept-disclosure` es una previsualización pura del manifiesto de
  divulgación (campo a campo, con redacciones aplicadas) que no envía nada;
  ejecutar exige devolver el SHA-256 exacto de ese manifiesto. Ruta
  air-gapped con `--local-exe` bajo `df-process-safety`.
- Migración append-only `0018_assistance_audit.sql`: una fila inmutable por
  invocación con el contrato de auditoría completo y su evento en el ledger
  en la misma transacción; `ai audits` la expone.
- E2E con modelo local determinista: preview sin clave, digest incorrecto
  rechazado, ejecución aislada, sugerencia validada con riesgo y confianza
  recalculados, auditoría y ledger verificados; cloud sin clave falla
  cerrado tras el consentimiento.

#### Límites

- La IA explica y sugiere etiquetas sobre items de revisión; no puede
  ejecutar, planificar ni aprobar nada, y ninguna sugerencia lleva acción.
- Deuda declarada: validación de clave en primer uso real, un caso de uso
  inicial y pantalla de escritorio pendiente (ADR-0034).

### Milestone 0.6 — Plugin Ecosystem (implementación local)

#### Añadido

- Migración append-only `0017_plugin_ecosystem.sql`: registros firmados de
  componentes (manifiesto, SHA-256, bytes, clave y firma Ed25519), runs
  direccionados por configuración y findings inmutables validados por
  triggers al sellar (ADR-0033).
- Orquestación de proyecto en `df-plugin`: todo lo leído del almacén se
  re-verifica (firma, hash, manifiesto, ABI, compilación) antes de
  ejecutarse; sujetos = contenidos únicos del snapshot analizado, paginados
  y acotados por `max_subjects` con sondeo del sujeto extra; trap, límite o
  salida malformada cuentan como sujeto fallido visible.
- Política de capacidades del operador: la fachada concede
  `SubjectMetadata` por defecto y reserva `SubjectText` a `--grant-text`;
  el host no concede nada por sí mismo.
- Fachada (`register_plugin`, `list_plugins`, `run_plugins`,
  `plugin_report`) y CLI (`plugin register|list|run`, `report plugins`).
  Eventos `PLUGIN_REGISTERED`, `PLUGIN_RUN_STARTED` y
  `PLUGIN_RUN_COMPLETED` en el ledger.
- E2E real: el ejemplo firmado `metadata-reporter` se registra, ejecuta
  sobre 2 sujetos, produce 2 findings INFO sellados y se reutiliza por
  digest; un componente manipulado tras la firma se rechaza sin almacenar.

#### Límites

- Los findings son afirmaciones del plugin ligadas a su identidad firmada;
  nunca autorizan una operación. `SubjectText` es concedible pero aún no se
  puebla desde las representaciones M0.4; la revocación de registros queda
  como decisión futura append-only.

### Milestone 0.5 — Media Intelligence (implementación local)

#### Añadido

- Migración append-only `0016_media_intelligence.sql`: runs de medios
  direccionados por el SHA-256 de su configuración serializada, evidencia
  por contenido (`media_evidence`) y relaciones de revisión
  (`media_relations`) selladas al completar. Los triggers exigen evidencia
  `EXTRACTED` en ambos lados de una relación, par ordenado y run `RUNNING`;
  el sellado valida contadores contra filas reales.
- Orquestación de proyecto en `df-media`: selección paginada de contenidos
  multimedia por extensión normalizada, verificación de fingerprint y
  SHA-256 antes y después de leer (fuente cambiada = conflicto duro),
  análisis reanudable por contenido y comparación por pares acotada con
  sondeo de un par extra (`pair_cap_reached` = cola real omitida).
- Relaciones `IMAGE_PERCEPTUAL_MATCH`, `AUDIO_ACOUSTIC_MATCH` y
  `VIDEO_PERCEPTUAL_MATCH` con score en millonésimas y evidencia literal de
  comparación. `automatic_action: true` es irrepresentable en el contrato.
- Fachada (`analyze_media`, `media_report`, resumen en `project_status`),
  CLI (`media --ffmpeg --image-worker --max-pairs`, `report media`) y
  sección M0.5 del escritorio con estados pendiente/sellado accesibles.
  El worker de imagen embebido se resuelve solo junto al ejecutable; FFmpeg
  solo por ruta absoluta explícita (ADR-0032).
- Prueba E2E con el worker aislado real: dos rediciones JPEG del mismo
  material se relacionan, la imagen ajena no, el run se sella y se
  reutiliza por digest, y sin workers el fallo es evidencia explícita
  `WORKER_UNAVAILABLE` con el run sellado igualmente.

#### Límites

- La selección es por extensión, no por sniffing de contenido, y las
  comparaciones son por pares dentro de cada tipo con techo explícito.
- Una coincidencia perceptual señala posibles rediciones para revisión
  humana; nunca autoriza eliminación, consolidación ni operación de plan.

### Rendimiento y robustez del motor (transversal)

#### Cambiado

- La base de proyecto se abre en modo WAL con `synchronous=FULL`,
  `trusted_schema=off` (el modelo de amenazas asume un `.sqlite` que pudo
  manipular un atacante) y `busy_timeout` de 5 s para que CLI y escritorio
  sobre el mismo proyecto esperen en vez de fallar con BUSY. Un commit sigue
  siendo durable al volver; en memoria y en sistemas de archivos sin WAL todo
  sigue funcionando con el journal previo.
- El hash persiste por lotes: una transacción por tanda de trabajos en lugar
  de un commit (y sus fsync) por archivo. La cola persistente ya hacía esto
  seguro — un corte pierde como mucho los resultados no confirmados, que se
  recalculan al reanudar (§14, regla 13). Los fallos por archivo siguen
  siendo datos del trabajo, nunca abortan la tanda.
- Migración `0015_hash_queue_index.sql`: índice que cubre filtro y orden de
  `pending_hash_jobs`; antes cada tanda reordenaba todos los PENDING
  restantes (trabajo cuadrático en runs grandes). El buffer de lectura se
  reserva una vez por run, no por archivo.
- Evidencia medida con el corpus sintético en release: la fase de hash pasa
  de 4,24 ms/archivo (run de 100 000) a 0,17 ms/archivo (20 000 archivos en
  3,4 s), ~25×; el pipeline completo de 20 000 archivos termina en 107 s con
  veredicto `COMPLETED` y ledger válido.

### Milestone 0.4 — Content Intelligence (implementación local)

#### Añadido

- Migración append-only `0014_content_intelligence.sql` y contratos tipados
  para runs reanudables, representaciones documentales, sujetos/segmentos,
  adjuntos, entradas ZIP virtuales, correo/hilos y registro inmutable de
  artefactos Tantivy/Parquet. El SHA-256 del JSON de límites se recalcula y la
  versión del extractor forma parte de toda identidad de reutilización.
- `df-extract`: extracción determinista de TXT, HTML, DOCX, EML y ZIP, con
  normalización Unicode, hashes y segmentación; límites absolutos de entrada,
  texto, entradas, bytes, ratio y nesting; preflight ZIP, rutas virtuales
  seguras, CRC/tamaño y cero materialización en disco.
- `df-extract-worker`: único binario que enlaza `pdf-extract`/`lopdf`. PDF de
  nivel superior, adjuntos EML y entradas ZIP viajan por un protocolo acotado
  y se ejecutan en Windows bajo Job Object de un proceso, memoria, deadline y
  kill-on-close. Ausencia, timeout o overflow producen evidencia explícita
  `LIMITED`/`FAILED`, sin fallback in-process.
- `df-search`: índice Tantivy reconstruible sobre texto, ruta, contexto,
  correo y metadata; consultas y snippets acotados. Directorios, meta y
  segmentos quedan arrendados contra sustitución durante hash y lectura; los
  lockfiles mutables de Tantivy se tratan como estado operativo fuera del
  digest.
- `df-query` y `df-query-worker`: snapshot Parquet versionado sin texto
  completo y SQL DataFusion read-only en proceso aislado. DDL/DML/statements,
  spill y familias opcionales de funciones están desactivados; memoria,
  tiempo, filas, celdas y salida tienen techos duros.
- `df-process-safety`: sidecars explícitos absolutos, sin `PATH`/entorno ni
  reparse points, con executable lease, Job Object y stdout/stdin acotados.
- Fachada y CLI para `content extract|fail|build|search|query`, replay y
  reutilización; códigos no exitosos para resultados limitados/fallidos.
  Escritorio Tauri/React con los mismos cinco flujos, estados asíncronos,
  resultados plain-text y tabla SQL accesible.
- Pruebas de formatos/overflow/ZIP hostil, EML→PDF, ZIP→PDF, workers
  timeout/protocolo/memoria, configuración dirigida por digest, integridad
  check/use, consulta aislada, flujo E2E y estados UI.

#### Límites

- Windows es el único backend con aislamiento de proceso y leases fuertes.
  PDF y SQL fallan cerrados en otras plataformas hasta M0.8.
- Texto e índices son evidencia derivada reconstruible. No prueban significado,
  no cambian un plan y no permiten ninguna acción destructiva.

### Milestone 0.3 — Similarity and Versioning (implementación local)

#### Añadido

- Crate `df-similarity`: FastCDC v2020 streaming con perfil inicial
  16/64/256 KiB, BLAKE3 por chunk, verificación SHA-256 completa y fingerprint
  pre/post. El origen se abre solo en lectura y una fuente modificada revierte
  toda la evidencia del contenido.
- Migración `0013_content_similarity.sql`: runs configurables, chunks
  normalizados, membresías ordenadas, firmas MinHash, bandas LSH, candidatos y
  relaciones `LIKELY_VERSION`, `TRUNCATED_VARIANT`, `RECOMPOSED_CONTENT` y
  `SIMILAR_CONTENT`. La evidencia global es append-only y cada run queda
  sellado al completarse.
- Generación de candidatos acotada mediante buckets LSH y fallback de chunks
  poco frecuentes. Se sondea exactamente un par más que el límite para que
  `candidate_cap_reached` signifique que existe una cola omitida, no solo que
  el recuento coincide casualmente con el máximo.
- Similitud exacta de multiconjuntos ponderada por bytes
  (`shared_bytes / union_bytes`). MinHash solo localiza candidatos y su
  estimación queda como evidencia secundaria; SHA-256 sigue siendo la única
  identidad.
- Reanudación por marcador de contenido y digest de configuración. Cambiar
  umbrales crea otro run reproducible y reutiliza chunks/firmas cuando el
  contrato de fragmentación no cambia. La reanudación verifica además todos
  los campos persistidos contra la configuración dirigida por el digest.
- Fachada, CLI (`similarity`, `report similarities`) y escritorio comparten el
  mismo DTO y exponen algoritmo, configuración exacta y digest. La CLI permite
  fijar umbral, mínimos de chunks/bytes y techo de candidatos; la vista muestra
  los parámetros junto a las relaciones y advierte que nunca autorizan
  borrado, consolidación ni una operación de plan.
- Pruebas de variantes sintéticas, separación identidad/similitud, cancelación
  y replay, cambio de umbral sin releer el origen, techo de candidatos,
  transacciones por contenido, límites de offsets/tamaños, evidencia LSH
  completa y sellado de runs. Benchmark manual de 256 MiB: working set
  acotado a dos chunks máximos más 1 KiB de firma.

#### Límites

- M0.3 relaciona bytes y tiempo de filesystem; todavía no interpreta texto,
  correo, contenedores ni formatos multimedia. Esas capacidades pertenecen a
  M0.4/M0.5.
- Los límites de buckets y candidatos aceptan falsos negativos y los hacen
  visibles. Ninguna relación automática equivale a una versión histórica
  confirmada ni cambia el plan.

### Milestone 0.2 — Structural Intelligence (objetivo 0.2.0)

#### Añadido

- Firmas Merkle BLAKE3 para carpetas completas y conjuntos
  `EXACT_TREE_CLONE` sobre identidades SHA-256 ya inventariadas
  (`0006_structure.sql`, ADR-0023).
- Clasificación determinista de carpetas `GENERIC`/`PROTECTED`/`NEUTRAL`,
  representante lógico explicado para cada conjunto de duplicados y perfiles
  declarativos embebidos `generic` y `legal` (`0007_contexts.sql`,
  `0008_representatives.sql`, ADR-0024–0026). Un id de perfil desconocido se
  rechaza en creación, apertura y análisis.
- Relaciones estructurales `PARTIAL_TREE_CLONE` y `TREE_EMBEDDED` sobre
  conjuntos de contenidos exactos (`0009_tree_relations.sql`, ADR-0027).
  Solo participan ramas completas; se excluyen ancestros de la misma raíz,
  componentes presentes en más de 32 carpetas y carpetas con menos de dos
  contenidos; los candidatos son estables y están limitados a 200 000. Se
  persisten recuentos de contenido exclusivo en ambos lados.
- Esquema de perfil `1.1.0` con reglas ordenadas y versionadas sobre el nombre
  de archivo. Sus únicas acciones son `COPY_ACTIVE`, `COPY_REVIEW`,
  `COPY_SEPARATED` y `COPY_TEMPORARY`; no existe acción declarativa
  destructiva (ADR-0028).
- Evidencia append-only para coincidencias de reglas, anomalías estructurales,
  cola de revisión y decisiones humanas justificadas
  (`0010_structural_review.sql`). El planner consume la última decisión; una
  revisión pendiente conserva la aparición como `COPY_REVIEW`.
- Anomalías deterministas para mismo nombre con contenido distinto, identidad
  visual de ruta degradada, entradas no leídas, rutas extremas, árboles
  parciales con contenido exclusivo y árboles embebidos.
- Marcador `analysis_completions` y evento único
  `STRUCTURAL_ANALYSIS_COMPLETED`. Los informes de duplicados, clones,
  relaciones, contextos, anomalías y revisión fallan cerrados hasta que el
  snapshot tenga marcador final y estado estable (ADR-0029).
- La migración `0011_derived_evidence_seal.sql` sella por snapshot duplicados,
  firmas, clones, contextos, relaciones y representantes tras ese marcador:
  SQLite rechaza `INSERT`, `UPDATE` y `DELETE`, mientras un snapshot nuevo y
  las decisiones humanas append-only permanecen operativos (ADR-0029).
- La migración `0012_execution_partial_lease.sql` persiste token aleatorio e
  identidad física de los parciales. La identidad se captura del handle
  creado con `create_new`; solo `RUNNING` + token + identidad coincidente
  permiten recuperación automática.
- CLI: `report tree-relations`, `report anomalies`, `review list` y
  `review decide`; `project status` y la app de escritorio muestran un resumen
  estructural M0.2.
- La multiplicidad distingue contenedores pasa-through de auto-injertos. Un
  ancestro solo se suprime cuando su conjunto de contenidos **y su total de
  apariciones** coinciden con los de la descendiente (como `Backup/` con un
  único expediente dentro); se reporta la carpeta más profunda y el evento
  registra `pass_through_suppressed`. Si el conjunto coincide pero el ancestro
  acumula apariciones adicionales porque contiene otra copia completa, se
  persiste como `REPEATED_COMPONENT_ONLY`, no como clon accionable
  (ADR-0027 §4).
- Generador de corpus sintético determinista `tools/df-corpus` y prueba de
  escala del pipeline completo (`cargo test -p df-corpus --release --
  --ignored scale`), cerrando los criterios 1 y 10 de M0.1: 100 000 archivos
  de crear a verificar con veredicto `COMPLETED`, origen intacto y ledger
  válido. El generador rechaza destinos no vacíos y crea archivos sin
  reemplazo; la integridad del origen cubre rutas, tipos y SHA-256, no solo
  recuento y tamaño (`docs/testing/corpus-and-scale.md`).

#### Cambiado

- Los temporales de copia se llaman ahora
  `.dataforge-partial-<operation-id>-<lease-token>` y no repiten el nombre
  original, de modo que incluso un componente NTFS de 255 unidades UTF-16
  deja espacio para el protocolo atómico. Planner y executor comparten un sufijo de colisión
  determinista y acotado: recorta el stem cuando hace falta y conserva la
  extensión completa siempre que cabe.
- La planificación acepta cinco políticas de duplicado. `REPORT_ONLY` sigue
  siendo la opción segura por defecto; las políticas de consolidación son
  opt-in, conservan todo contexto desconocido y nunca atraviesan una frontera
  protegida. Reglas y revisión pueden seleccionar operaciones de copia, pero
  no omitir una aparición ambigua.
- `analyze` se puede reanudar desde `ANALYZING`; `plan create`, desde
  `PLANNING`; y `plan approve`, desde `PLAN_REVIEW`. Un plan `READY` ya
  persistido se valida y reutiliza sin crear otra versión, y una aprobación ya
  persistida reutiliza el mismo manifiesto/hash sin duplicar el evento
  (ADR-0029).
- El análisis estructural termina ahora después de duplicados, firmas,
  contextos, relaciones, representantes, reglas y anomalías; el antiguo evento
  `ANALYSIS_COMPLETED` de la etapa de duplicados no se usa como marcador final.

#### Límites

- M0.2 trabaja con hashes exactos, estructura de carpetas y nombres declarados.
  No extrae el contenido documental, no infiere asuntos por significado y no
  construye un grafo semántico.
- Las relaciones entre árboles son evidencia de conservación/revisión. No
  consolidan automáticamente una rama y aceptan falsos negativos por sus
  límites explícitos de candidatos y componentes ubicuos.
- Las fronteras del perfil `legal` son coincidencias de nombre exactas o
  prefijos acotados; no detectan un expediente cuyo nombre no contenga un
  marcador declarado.

### Hardening de seguridad del sistema de archivos (v0.1.1-dev)

Endurece el núcleo para poder probarlo sobre colecciones reales supervisadas.
No añade funcionalidad de producto. Modelo de amenazas completo en
[`docs/threat-model/filesystem-hardening.md`](docs/threat-model/filesystem-hardening.md).

- **Frontera segura del sistema de archivos** (crate nuevo `df-fs-safety`,
  ADR-0017): toda escritura pasa por él. El output root se valida y se
  identifica **físicamente** (volume serial + file id) antes de escribir y se
  revalida durante la ejecución; los destinos se resuelven **componente a
  componente** rechazando cualquiera que sea reparse point (symlink, junction o
  mount point). Sustituye `create_dir_all` y `File::create` por equivalentes
  que comprueban cada nivel. Motivo: validar que una ruta es relativa y sin
  `..` es *texto*, y el texto no dice nada del disco — una junction preexistente
  dentro de la salida redirigía la escritura fuera de ella.
- **Finalize sin reemplazo real** (ADR-0021): `MoveFileExW` **sin**
  `MOVEFILE_REPLACE_EXISTING`. Corrige una ventana real de sobrescritura
  silenciosa: en Windows `std::fs::rename` **sí sobrescribe**, y el
  `destination.exists()` previo era una comprobación TOCTOU — el código
  afirmaba una garantía que en esta plataforma no tenía (regla 3).
- **El verificador nunca sigue enlaces** (§28.2): recorrido con
  `symlink_metadata`, reparse points reportados y jamás traspasados, ciclos
  cortados por identidad física, y errores de lectura convertidos en hallazgos
  (`OUTPUT_REPARSE_POINT`, `OUTPUT_SUBTREE_UNREADABLE`) en vez de un `continue`
  silencioso. Antes podía leer **fuera** del output root y aun así certificarlo.
- **Manifiesto de ejecución inmutable** (migración `0004_execution_manifest`,
  ADR-0018): la aprobación congela el contrato completo —qué se lee, qué
  contenido se espera, dónde se escribe y qué operación corre— y el SHA-256 lo
  cubre entero. El executor ejecuta **solo** el manifiesto; las tablas de
  inventario vuelven a ser evidencia. Inmutabilidad impuesta por triggers.
  Antes, editar `content_objects.sha256` tras aprobar cambiaba lo ejecutado
  **sin mover el hash del plan** (la regla 10 era medio verdad).
- **Fingerprint físico v2** (ADR-0019): enum versionado `V1`/`V2`; v2 añade
  identidad física, `ChangeTime` de NTFS y atributos. Detecta la sustitución de
  un archivo por otro **del mismo tamaño y mismo mtime**, que v1 no veía. La
  comparación es un veredicto explícito (`SamePhysical`/`SameDegraded`/
  `Changed`), no `PartialEq`: identidad degradada **no** es "sin cambios", y v1
  y v2 nunca se declaran equivalentes. Los tokens v1 existentes siguen
  leyéndose.
- **Rutas raw reversibles** (migración `0005_path_identity`, ADR-0020): se
  conservan las unidades UTF-16 exactas (BLOB LE; hex en el JSON del
  manifiesto). Display, comparación y raw son tres cosas distintas y solo la
  raw abre archivos. Antes, un nombre con un surrogate suelto —legal en
  Windows— podía quedar inabrible o, peor, abrir **otro** archivo.
- **Creación atómica de proyectos y marker endurecido** (ADR-0022): el proyecto
  se construye en `<dir>.init-<uuid>` y se finaliza con un rename atómico; el
  marker se escribe el último y solo tras el integrity check. Un fallo no deja
  medio proyecto y el reintento funciona; **nunca** se limpia una carpeta
  preexistente del usuario. El marker deja de ser autoritativo para la ruta de
  la base (en Windows `join` con ruta absoluta descartaba la base y permitía
  redirigir SQLite fuera del proyecto), y `schema_version` gobierna la apertura
  con política explícita para versión futura, antigua o manipulada.
- CI: jobs Windows específicos de hardening, tests de manipulación y
  compatibilidad de migraciones.
- `cargo deny` vuelve a estar verde: llevaba roto desde M0.0 sin detectarse
  porque la CI nunca había llegado a ejecutarse. Los wildcards se eliminan
  dando versión explícita a las dependencias internas (sin excepción de
  configuración); los cinco advisories `unmaintained` de `unic-*` —que llegan
  transitivamente desde Tauri— se ignoran **uno a uno**, documentados y con
  condición de retirada y fecha de revisión.

### Limitaciones de este incremento

- **Windows es la única plataforma con seguridad implementada.** En el resto,
  la ejecución se **bloquea** en lugar de fingir garantías (regla 19).
- NAS/UNC sigue **experimental**: sin `file_id` la identidad es *degradada* y no
  se puede descartar sustitución.
- La garantía frente a quien pueda editar la base es de **detección**, no de
  prevención.
- Sin durabilidad garantizada ante fallo físico del hardware.
- Queda ventana TOCTOU residual entre validación y escritura: se reduce a
  "falla, no pisa", no se elimina.

## [Anterior] — Milestone 0.1 "Safe Inventory Core"

### Añadido

- Migración `0002_inventory`: tablas `scan_runs`, `folders`,
  `path_occurrences`, `content_objects`, `occurrence_content` y `hash_jobs`
  (RFC-0001 §10.1), STRICT y con claves foráneas.
- `df-scan`: validación de orígenes (§12.1) y escáner seguro (§13) — cola
  iterativa, reparse points registrados y nunca seguidos, rutas largas
  Windows (`\\?\`), nombres no-Unicode marcados, errores parciales
  persistidos, batches transaccionales, cancelación segura.
- `df-hash`: fingerprint físico v1, BLAKE3 + SHA-256 en una sola pasada de
  lectura, invalidación pre/post (`SOURCE_CHANGED`, §14.5) y cola de
  trabajos reanudable (`hash_jobs`).
- Duplicados exactos (mismo tamaño + SHA-256, §15) como informe de
  evidencia, sin proponer acciones.
- Eventos de auditoría del pipeline: `SCAN_STARTED/COMPLETED/CANCELLED/
  FAILED`, `HASH_STARTED/COMPLETED/PAUSED`.
- `df-facade`: `scan_project`, `hash_project`, `duplicate_report`,
  `verify_audit`; `ProjectStatus` incluye snapshot e inventario.
- CLI: `dataforge scan`, `dataforge hash`, `dataforge report duplicates`,
  `dataforge audit verify` (con `--json` y códigos de salida §33, incluido
  `3 partial completion`).
- Desktop: la vista de estado muestra snapshot, inventario y progreso de
  hashing reales.
- ADR-0015 con las decisiones del incremento.
- Migración `0003_planning`: `duplicate_sets`, `plans`, `plan_operations`
  (congeladas por trigger al aprobar), `operation_results` (append-only),
  `verification_runs` y `verification_findings`.
- `df-planner`: análisis (materializa duplicate_sets, §15), generación de
  plan con cobertura completa (§26.2) bajo política `REPORT_ONLY` —
  `COPY_ACTIVE`, `CREATE_DIRECTORY`, `NO_ACTION`, `BLOCKED`,
  `COPY_WITH_SUFFIX` para colisiones —, validación §26.5 y aprobación con
  serialización canónica + SHA-256 (§26.4).
- `df-executor`: protocolo por archivo del §27.1 (fingerprint pre/post,
  parcial `.dataforge-partial-<op>-<lease>`, copia en streaming con doble hash,
  flush, comparación, rename atómico), colisiones §27.3, errores tipados
  §27.5, reanudación §27.4 y cancelación segura.
- `df-verifier`: verificación independiente §28 — re-hash de cada destino,
  cobertura de ejecución, plan no manipulado (re-serialización canónica),
  parciales huérfanos, archivos no registrados y origen sin cambios;
  veredicto `COMPLETED` / `COMPLETED_WITH_WARNINGS` / `FAILED`.
- Eventos: `ANALYSIS_COMPLETED`, `PLAN_CREATED`, `PLAN_APPROVED`,
  `EXECUTION_COMPLETED/PAUSED`, `VERIFICATION_COMPLETED`.
- CLI: `dataforge analyze`, `plan create/validate/approve`, `execute`,
  `verify` — el pipeline completo del RFC §33 para 0.1.
- ADR-0016 con las decisiones del incremento de plan/ejecución/verificación.

### Seguridad

- El escáner y el hasher abren el origen exclusivamente en lectura; los
  tests verifican que el origen no cambia tras el pipeline completo.
- El executor nunca sobrescribe (rename que falla si el destino existe,
  `SKIP_REPRESENTED`/sufijo determinista en colisiones) y el único borrado
  del código son sus propios archivos parciales fallidos (ADR-0016).
- Un plan aprobado es inmutable por trigger SQL y su manipulación offline
  se detecta criptográficamente en la verificación (`PLAN_TAMPERED`).

## [0.0.1-dev] — 2026-07-13 — Milestone 0.0 "Repository Foundation"

### Añadido

- Monorepo: workspace Cargo (7 crates) + workspace pnpm.
- `df-error`: errores tipados y códigos de salida (RFC-0001 §33).
- `df-domain`: IDs tipados (UUIDv4), `Project`, `ProfileRef`, `SourceRoot`
  (solo lectura por construcción), `Snapshot`, `AuditEvent`, `Actor` y la
  máquina de estados completa de RFC-0001 §11 con sus invariantes.
- `df-ledger`: JSON canónico, timestamps canónicos, construcción y
  verificación de cadenas de eventos SHA-256 (genesis, secuencia contigua,
  envelope que cubre metadatos).
- `df-db`: SQLite (rusqlite bundled), migración `0001_foundation` (tablas
  STRICT `projects`, `source_roots`, `snapshots`, `audit_events` +
  `schema_migrations`), migraciones con checksum verificado en apertura,
  triggers append-only sobre `audit_events`, repositorios transaccionales
  (crear proyecto, transición de estado, eventos) y pasada de integridad.
- `df-facade`: `create_project`, `open_project`, `project_status`;
  validación de rutas disjuntas; marker `project.dataforge.json` versionado.
- CLI `dataforge`: `project create`, `project status`, `--json`, códigos de
  salida 0/1/2/4/5.
- Desktop `DataForge Desktop` (Tauri 2 + React 19 + TS strict): pantallas
  de inicio, crear proyecto, abrir proyecto y estado con integridad; sin
  lógica crítica en la UI.
- Documentación: RFC-0001 en `docs/rfcs/`, ADR-0001..0003 y ADR-0011..0014,
  system overview, threat model inicial, guías de contribución y entorno.
- Bootstrap reproducible: `scripts/*.ps1` idempotentes + informe de entorno.
- Skills del repositorio en `.codex/skills/`.
- CI (GitHub Actions, Windows): fmt, clippy `-D warnings`, tests, build CLI,
  typecheck/build frontend, `cargo audit` + `cargo deny`.
- Gobernanza: licencias MIT/Apache-2.0, README, CONTRIBUTING (DCO),
  SECURITY, GOVERNANCE, Código de Conducta, plantillas de issues y PR.

### Seguridad

- Sin rutas de código de borrado ni sobrescritura; orígenes de solo lectura
  por política, reforzado con `CHECK` en SQL.
- Ledger append-only con verificación criptográfica y tests de manipulación.
