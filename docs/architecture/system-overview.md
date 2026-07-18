# DataForge — visión general del sistema

Estado del documento: refleja M0.2–M0.4 implementados localmente. La
arquitectura objetivo completa está en
[RFC-0001 §7](../rfcs/RFC-0001-dataforge-foundation-and-roadmap.md).

## Capas

```text
┌────────────────────────────┐   ┌──────────────────────────┐
│     DataForge Desktop      │   │      dataforge (CLI)     │
│ Tauri 2 + React + TS strict│   │  clap, texto y --json    │
└─────────────┬──────────────┘   └────────────┬─────────────┘
              │  comandos IPC                │ llamadas directas
┌─────────────▼───────────────────────────────▼─────────────┐
│                        df-facade                          │
│ proyecto/marker, DTOs, informes y única API de clientes  │
└───────┬────────────┬────────────┬────────────┬────────────┘
        │            │            │            │
┌───────▼──────┐ ┌───▼────┐ ┌────▼──────┐ ┌──▼───────────┐
│   df-scan    │ │df-hash │ │df-planner │ │ df-executor  │
│ inventario   │ │ hashes │ │análisis + │ │ copia segura │
│ inmutable    │ │ dobles │ │ planes     │ │ reanudable   │
└───────┬──────┘ └───┬────┘ └────┬──────┘ └──┬───────────┘
        │            │            │            │
        │       ┌────▼────────────▼────────────▼──────┐
        │       │                 df-db               │
        └──────►│ SQLite, migraciones, repositorios,  │
                │ análisis derivado e integridad      │
                └───────┬──────────────────┬──────────┘
                        │                  │
                ┌───────▼──────┐   ┌──────▼──────────┐
                │  df-ledger   │   │ df-fs-safety   │
                │ eventos SHA- │   │ frontera física│
                │ 256 canónicos│   │ de escritura   │
                └───────┬──────┘   └──────┬──────────┘
                        │                  │
                ┌───────▼──────────────────▼──────────┐
                │              df-domain              │
                │ tipos puros, estados, políticas y   │
                │ vocabulario estructural             │
                └─────────────────────────────────────┘
```

`df-verifier` vuelve a leer la salida y el manifiesto aprobado de forma
independiente después de `df-executor`. `df-error` aporta errores tipados y
códigos de salida a todas las capas.

`df-similarity` es otro motor detrás de `df-facade`: lee el snapshot ya
hasheado/analizado, usa `df-fs-safety` para capturar fingerprints y delega toda
persistencia a `df-db`. No depende de planner/executor y no puede crear
operaciones; esta separación hace estructural la regla “similitud ≠ permiso”.

M0.4 añade `df-extract`, `df-search` y `df-query` detrás de la misma fachada.
Los parsers PDF y las consultas SQL de clientes cruzan protocolos internos
versionados hacia `df-extract-worker` y `df-query-worker`; `df-process-safety`
los limita con primitivas del sistema operativo. SQLite conserva toda evidencia
canónica; Tantivy y Parquet son artefactos derivados reconstruibles.

## Reglas de dependencia

- `df-domain` es puro: no hace I/O ni SQL.
- Solo `df-db` contiene SQL y persiste hechos/evidencias del motor.
- Toda escritura de ejecución pasa por `df-fs-safety`; el origen se abre en
  lectura y nunca se convierte en destino.
- `df-facade` es la frontera estable: valida el directorio/marker del proyecto
  y ofrece operaciones y DTOs serializables. CLI y desktop no abren SQLite.
- La UI no contiene decisiones críticas: presenta estado, diagnóstico e
  integridad y delega toda mutación al motor.
- SQLite es la autoridad transaccional local; el ledger registra las
  transiciones y decisiones, pero no sustituye a la base.

## Datos en disco

```text
<proyecto>/
├── project.dataforge.json      # marcador versionado, validado, no autoritativo
├── state/
│   └── dataforge.sqlite        # única fuente de verdad transaccional
└── audit/                      # raíz de auditoría configurada por el proyecto
```

La salida verificada vive en `output_root`, fuera del proyecto y de todas las
raíces de origen. La creación del proyecto usa un directorio staging hermano y
publica marker + base conjuntamente mediante rename.

## Esquema implementado

Las migraciones son aditivas y sus checksums se verifican en cada apertura:

- `0001`–`0003`: proyecto, inventario y planificación;
- `0004`–`0005`: manifiesto inmutable e identidad raw de rutas;
- `0006`–`0008`: firmas/clones exactos, contextos y representantes;
- `0009`: relaciones parciales/embebidas entre árboles;
- `0010`: coincidencias de reglas, anomalías, revisión y marcador de análisis
  completo;
- `0011`: sellado por snapshot de toda evidencia automática derivada tras el
  marcador final;
- `0012`: lease de parcial con token aleatorio e identidad física capturada
  desde el handle que ganó `create_new`;
- `0013`: runs de similitud, chunks/membresías, MinHash/LSH, candidatos y
  relaciones de contenido selladas;
- `0014`: runs de extracción, representaciones/segmentos, correo, adjuntos,
  entradas ZIP virtuales e índices/snapshots derivados registrados.

Los hechos automáticos de `0010` usan ids derivados estables e inserciones
idempotentes. `0011` impide insertarlos, modificarlos o borrarlos una vez
completado el análisis; las decisiones humanas y el marcador final siguen
siendo streams append-only separados. `0012` permite recuperar un parcial
solo si estado, token e identidad física coinciden; token o nombre solos no
constituyen propiedad.

## Flujo implementado

1. **Crear/abrir.** La fachada valida rutas disjuntas, perfil y
   marker. La base se crea en staging, pasa integridad y se publica de forma
   atómica. Un perfil desconocido falla cerrado.
2. **Escanear.** `df-scan` registra un snapshot, carpetas, apariciones, errores
   y reparse points sin seguirlos. El origen no se modifica.
3. **Hashear.** `df-hash` calcula BLAKE3 + SHA-256 en una pasada, compara el
   fingerprint antes/después y conserva una cola reanudable.
4. **Analizar.** `df-planner` orquesta, en este orden:
   duplicados exactos; firmas y clones exactos; contextos; relaciones de
   árboles; representantes; reglas declarativas; anomalías; y marcador
   `STRUCTURAL_ANALYSIS_COMPLETED`. Solo entonces transiciona a `ANALYZED`.
5. **Relacionar (M0.3, opcional).** `df-similarity` reabre cada contenido
   elegible desde su ruta raw, valida fingerprint, fragmenta con FastCDC y
   publica atómicamente chunks + MinHash. SQLite genera candidatos acotados;
   cada uno se recalcula desde el multiconjunto exacto y el run se sella con
   su evento. El estado del proyecto no retrocede ni el plan cambia.
6. **Interpretar contenido (M0.4, opcional).** `df-extract` reutiliza evidencia
   por contenido+versión+configuración, normaliza documentos/correo/ZIP y
   sella el run. PDF se delega al sidecar. Desde el run sellado se reconstruyen
   Tantivy y Parquet; búsqueda y SQL verifican digest y leases antes de usarlo.
7. **Revisar.** Los hallazgos ambiguos viven en una cola. Una decisión exige
   justificación y se añade al historial; la más reciente guía planes futuros.
8. **Planificar.** Cada carpeta/aparición queda cubierta por una operación. La
   política de duplicados es explícita y la guía de reglas/revisión solo elige
   operaciones de copia. Incertidumbre y fronteras protegidas conservan datos.
9. **Aprobar.** El plan pasa por validación, congela un manifiesto de ejecución
   completo y firma su serialización canónica con SHA-256.
10. **Ejecutar.** `df-executor` copia desde el manifiesto, usa parciales,
   doble hash, fingerprints y finalize sin reemplazo.
11. **Verificar.** `df-verifier` re-hashea destinos, comprueba cobertura,
   manifiesto, parciales, archivos ajenos y origen; emite el veredicto final.

## Inteligencia estructural M0.2

- Las firmas Merkle solo son válidas para subárboles completos.
- Los clones exactos agrupan firmas completas iguales.
- Las relaciones parciales/embebidas comparan conjuntos de identidades exactas
  con límites explícitos; persisten cuánto contenido es exclusivo de cada lado.
- Si el límite de pares distintos corta la generación,
  `candidate_cap_reached` queda sellado y visible en estado/CLI/desktop: las
  relaciones siguen siendo conservadoras, pero no se presentan como
  exhaustivas.
- Los perfiles `generic` y `legal` clasifican nombres de carpetas. `legal`
  añade fronteras que ninguna política de duplicado puede disolver.
- Las reglas de perfil solo inspeccionan nombres de archivo y solo proponen
  cuatro variantes de copia.
- Las anomalías son diagnósticos explicados; una revisión pendiente se copia a
  revisión en vez de representarse silenciosamente.
- Los informes de duplicados, árboles, contextos, anomalías y revisión exigen
  el marcador final del snapshot y un estado estable posterior al análisis.

## Similitud y versionado M0.3

- `v2020::StreamCDC` trabaja con perfil versionado 16/64/256 KiB. Memoria de
  proceso: O(chunk máximo), nunca O(archivo) ni O(corpus).
- `chunks` normaliza BLAKE3+longitud sin almacenar bytes;
  `chunk_memberships` conserva ordinal/offset. Un contenido se publica entero
  en una transacción y después es append-only.
- La firma MinHash determinista y sus bandas LSH solo generan candidatos. Un
  fallback de chunks poco frecuentes reduce falsos negativos por bandas.
- Buckets y pares tienen límites persistidos. El motor prueba un candidato más
  que el máximo antes de marcar `candidate_cap_reached`, por lo que la bandera
  implica una cola realmente truncada.
- La cifra publicada siempre es exacta:
  `shared_bytes / (size_a + size_b - shared_bytes)`, con multiplicidad de
  chunks. SHA-256 sigue siendo la única prueba de identidad.
- Un digest identifica configuración + algoritmo. Repetirlo reanuda o devuelve
  el run sellado; cambiar solo umbrales crea otro run y reutiliza las firmas.
- CLI y escritorio muestran tipo, dirección temporal, porcentaje, chunks y
  bytes compartidos. La evidencia declara `automatic_action=false` y ninguna
  API la convierte en plan.

## Inteligencia documental M0.4

- Un digest SHA-256 cubre el JSON exacto de límites; `extractor_version` forma
  parte de la clave de run y representación. Un replay devuelve el run sellado
  o continúa desde el primer contenido sin evidencia.
- TXT/HTML/DOCX/EML/ZIP trabajan con bytes verificados del origen y techos
  absolutos. Adjuntos y entradas son sujetos virtuales con linaje; ZIP nunca se
  materializa. References/In-Reply-To tiene prioridad sobre el asunto al formar
  hilos básicos.
- `df-extract` no enlaza `lopdf`. `df-extract-worker` procesa todo PDF bajo Job
  Object; timeout, memoria, protocolo o output agotado quedan visibles como
  `LIMITED`/`FAILED`.
- Tantivy indexa texto/ruta/contexto; Parquet conserva metadata analítica, no
  texto completo. Ambos nacen solo de runs `COMPLETED`, registran schema+digest
  y se pueden reconstruir sin releer el origen.
- Leases fijan artefacto, ficheros y ancestros durante hash y consumo. Lockfiles
  de Tantivy permanecen mutables pero no sustituibles y no forman parte del
  digest de evidencia.
- SQL de CLI/desktop solo pasa por `df-query-worker`. DataFusion no permite
  DDL/DML/statements/spill y aplica límites de memoria, tiempo, filas, celdas y
  bytes; ausencia de sidecar falla cerrada.

## Recuperación

- `ANALYZING`: sin marcador final, se reejecutan etapas idempotentes sin
  repetir la transición inicial. Con marcador válido, se valida su scope,
  perfil, digest y resumen contra la evidencia sellada, se reconstruye el
  resultado y solo se completa `ANALYZING → ANALYZED`.
- `PLANNING`: se crea el plan que falte o se valida/reutiliza el `READY` ya
  persistido para el mismo snapshot y operaciones efectivas.
- `PLAN_REVIEW`: se persiste la aprobación pendiente o se verifica/reutiliza el
  manifiesto aprobado antes de completar la transición del proyecto.
- Scan, hash y ejecución mantienen sus propios protocolos de pausa/reintento.
- Similitud mantiene `RUNNING` si se cancela o falla antes del cierre. Las
  firmas completas se reutilizan; candidatos/relaciones del run se reconstruyen
  determinísticamente y solo `SIMILARITY_COMPLETED` vuelve visible el resumen.

Un snapshot sellado no se recalcula con otra `ANALYSIS_VERSION`: como varias
tablas derivadas no llevan versión por fila, una versión nueva del algoritmo
requiere crear un proyecto nuevo mediante el flujo soportado y ejecutar en él
`scan → hash → analyze`. El proyecto sellado en `ANALYZED` no retorna a
`scan`.

Estas garantías cubren caídas entre transacciones de un proyecto local. No son
un protocolo de coordinación distribuida entre varios escritores.

## Límites explícitos

M0.4 extrae e indexa documentos, pero no interpreta su significado ni
reconstruye automáticamente asuntos o expedientes. Sus textos son derivados;
las relaciones M0.3 proceden de chunks binarios y fechas y las de M0.2 de
hashes exactos, estructura y marcadores de nombre. Los límites pueden omitir
contenido/relaciones y ninguna evidencia autoriza consolidar archivos o ramas.

Las garantías de escritura segura siguen siendo Windows-first; NAS/UNC es
experimental. Véase
[Modelo de amenazas de filesystem](../threat-model/filesystem-hardening.md).
