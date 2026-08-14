# Modelo de amenazas — Filesystem Safety Hardening (v0.1.1-dev)

Estado: sigue siendo la base de seguridad; M0.2 añade análisis estructural y
M0.3 lectura de similitud, pero ninguno amplía plataformas ni relaja estas
garantías. Véase también [`initial.md`](initial.md).

Ámbito: el pipeline `scan → hash → analyze → similarity → plan → approve →
execute → verify` frente a un sistema de archivos **hostil o simplemente tramposo**.
Complementa [`initial.md`](initial.md), que cubre la fundación.

Este documento existe porque el objetivo de v0.1.1-dev es poder ejecutar el
pipeline sobre **colecciones reales supervisadas**. Antes de eso hay que
demostrar —no afirmar— que ninguna escritura escapa de la salida autorizada y
que lo aprobado es exactamente lo ejecutado.

## Modelo de atacante

No asumimos un atacante remoto. Asumimos que **el sistema de archivos de
origen es material heredado y no confiable**: discos rescatados, backups de
terceros, NAS compartidos, carpetas sincronizadas. El "atacante" puede ser:

- **A1 — Contenido heredado hostil o accidental**: junctions, symlinks y mount
  points que ya existían en el material, creados por otro software o por un
  usuario años atrás. Es el caso realista y el que más nos preocupa.
- **A2 — Proceso concurrente**: sincronizadores (OneDrive), antivirus,
  backups o el propio usuario tocando archivos mientras DataForge trabaja.
  Produce carreras (TOCTOU) sin intención maliciosa.
- **A3 — Manipulador local con acceso a la base**: alguien que edita el SQLite
  del proyecto entre `approve` y `execute` para cambiar qué se copia.

No está en alcance un atacante con privilegios de administrador ni capaz de
modificar el binario de DataForge: contra eso ninguna comprobación en proceso
sirve.

## Activos protegidos

1. **El origen** — inmutable (regla 1). Su alteración es el peor resultado.
2. **La salida documental** — todo lo escrito debe quedar bajo `output_root`
   y nada preexistente puede sobrescribirse (reglas 2 y 3).
3. **El contrato de ejecución** — lo aprobado (regla 10) debe ser exactamente
   lo ejecutado.
4. **La evidencia** — ledger y manifiesto: si mienten, el producto no vale.

## Amenazas

Cada amenaza lleva: actor, precondición, impacto, mitigación, riesgo residual
y el test que lo demuestra.

### T1 — Junction planting (escape del output)

- **Activo**: salida documental.
- **Actor**: A1 / A2.
- **Precondición**: un componente intermedio del destino ya existe dentro de
  `output_root` y es una junction hacia fuera (p. ej. `Salida\clientes` →
  `C:\DatosExternos`).
- **Impacto**: DataForge escribe fuera de la salida autorizada. Puede pisar
  datos ajenos al proyecto. Rompe la promesa central del producto.
- **Mitigación**: `df-fs-safety` resuelve el destino componente a componente
  rechazando cualquier reparse point existente; identidad del output root
  comprobada por handle antes de escribir.
- **Riesgo residual**: solo Windows en esta versión; en Linux/macOS la
  ejecución se bloquea (ADR-0017).
- **Test**: `junction_in_output_is_rejected` (requiere Windows).

### T2 — Symlink escape

- **Activo**: salida documental.
- **Actor**: A1.
- **Precondición**: un componente del destino es un symlink de directorio o
  de archivo.
- **Impacto**: igual que T1.
- **Mitigación**: idéntica a T1; el atributo `FILE_ATTRIBUTE_REPARSE_POINT`
  cubre symlink, junction y mount point sin distinguir.
- **Riesgo residual**: crear symlinks exige Developer Mode o privilegio; los
  tests que los crean se saltan con motivo explícito si no se puede.
- **Test**: `symlink_in_output_is_rejected` (requiere Windows + privilegio).

### T3 — Reparse race / TOCTOU destination swap

- **Activo**: salida documental.
- **Actor**: A2.
- **Precondición**: una carpeta normal se convierte en junction, o el destino
  aparece, **entre** la validación y la escritura.
- **Impacto**: escritura fuera de la salida, o sobrescritura de un archivo que
  no existía al validar.
- **Mitigación**: la validación no se hace "antes y ya"; el finalize usa una
  primitiva **no-replace** de plataforma, de modo que si el destino aparece en
  la ventana la operación falla con `DESTINATION_CHANGED` en lugar de pisarlo.
  La identidad del output root se revalida durante la ejecución.
- **Riesgo residual**: no se puede cerrar la ventana por completo sin mantener
  handles abiertos de toda la cadena; se reduce a "falla, no pisa".
- **Test**: `destination_appearing_before_finalize_fails_without_overwrite`.

### T4 — Overwrite silencioso por semántica de rename

- **Activo**: salida documental (archivos preexistentes).
- **Actor**: A2, o simplemente un destino ya presente.
- **Precondición**: `std::fs::rename` en Windows llama a `MoveFileExW` **con**
  `MOVEFILE_REPLACE_EXISTING`: **sobrescribe**. El código previo a v0.1.1
  solo se protegía con un `destination.exists()` anterior, que es TOCTOU.
- **Impacto**: violación directa de la regla 3 (no sobrescribir). Pérdida
  silenciosa de datos del usuario en la salida.
- **Mitigación**: `finalize_no_replace` usa `MoveFileExW` **sin** ese flag, de
  modo que el propio sistema falla si el destino existe. El `exists()` previo
  pasa a ser una optimización, no la garantía.
- **Riesgo residual**: en filesystems que no soporten rename atómico la
  garantía se degrada; se documenta y se bloquea la plataforma no soportada.
- **Test**: `finalize_no_replace_refuses_existing_destination`,
  `preexisting_destination_content_is_never_modified`.

### T5 — Tampered execution manifest / inventory / content identity

- **Activo**: contrato de ejecución.
- **Actor**: A3.
- **Precondición**: el executor resuelve en tiempo de ejecución qué leer y
  qué esperar mediante joins vivos contra `path_occurrences`, `source_roots` y
  `content_objects`. Editar esas tablas tras aprobar cambia lo ejecutado **sin
  cambiar el hash del plan**.
- **Impacto**: se ejecuta material distinto del aprobado; la aprobación deja
  de significar nada (rompe la regla 10).
- **Mitigación**: manifiesto de ejecución inmutable congelado en la
  aprobación, cubierto por el SHA-256 canónico del plan; triggers que
  prohíben `UPDATE`/`DELETE`; el executor ejecuta **solo** el manifiesto; la
  verificación recalcula el hash.
- **Riesgo residual**: quien pueda editar la base también puede borrarla; la
  garantía es de *detección*, no de prevención.
- **Test**: `tampering_manifest_fails_verification`,
  `changing_content_objects_after_approval_does_not_change_execution`.

### T6 — Sustitución de origen con mismo tamaño y mtime

- **Activo**: origen / integridad de la copia.
- **Actor**: A2.
- **Precondición**: `FileFingerprint v1` = `(size, mtime)`. Sustituir un
  archivo por otro de igual tamaño y fecha no se detecta.
- **Impacto**: se copia contenido distinto del hasheado, o se declara
  "sin cambios" algo que cambió.
- **Mitigación**: `FileFingerprint v2` incluye identidad física
  (`volume_serial` + `file_id`) cuando el filesystem la ofrece; el
  contenido se re-hashea y se compara con el esperado del manifiesto.
- **Riesgo residual**: si el filesystem no da identidad física, el
  fingerprint queda **degradado** y así se registra; no se presenta como
  identidad fuerte.
- **Test**: `same_size_same_mtime_replacement_is_detected`.

### T7 — Subárbol de salida ilegible / verificador que sigue enlaces

- **Activo**: evidencia de verificación.
- **Actor**: A1 / A2.
- **Precondición**: `walk_output` usa `read_dir` con `continue` silencioso
  ante error y `entry.metadata()` (que sigue enlaces).
- **Impacto**: el verificador puede leer **fuera** de la salida, entrar en
  bucles, o declarar íntegra una salida que no ha podido inspeccionar.
- **Mitigación**: recorrido con `symlink_metadata`, rechazo de reparse
  points, detección de ciclos por identidad física y hallazgos tipados en vez
  de `continue`.
- **Riesgo residual**: ninguno conocido dentro del alcance Windows.
- **Test**: `verifier_never_follows_links`, `unreadable_subtree_is_a_finding`.

### T8 — Partial finalize tras caída

- **Activo**: salida documental.
- **Actor**: A2 (corte de luz, kill).
- **Precondición**: caída entre el `sync` del parcial y el registro del
  resultado.
- **Impacto**: parcial huérfano, o copia finalizada sin resultado en la base.
- **Mitigación**: cada intento reserva un UUID y crea
  `.dataforge-partial-<operation-id>-<lease-token>` con `create_new`. Solo tras
  ganar la creación persiste un claim físico capturado desde **ese handle**.
  Reclaim exige `RUNNING` + token + identidad coincidente; abre con acceso de
  borrado, bloquea sustituciones, valida archivo regular/no-reparse y elimina
  por el mismo handle. Finalize aplica la misma regla y renombra por handle sin
  reemplazo. Reclaim(A) ocurre antes de emitir lease(B), por lo que una caída
  entre ambos conserva una prueba repetible. Un fallo I/O al limpiar mantiene
  `RUNNING` y el claim; identidad distinta/reparse se conserva como extranjera.
- **Riesgo residual**: una caída después de que `create_new` gane pero antes de
  persistir la identidad deja un `PARTIAL_LEFTOVER` sin claim. Se conserva
  deliberadamente: borrarlo sería indistinguible de borrar un squatter que
  ocupó el token exacto. VERIFY lo falla y requiere inspección/limpieza manual.
  Tampoco hay garantía absoluta ante fallo físico del disco (ADR-0021).
- **Tests**: `crash_with_partial_file_resumes`,
  `create_new_collision_then_crash_never_authorizes_foreign_deletion`,
  `a_partial_substituted_after_claim_is_never_deleted`,
  `blocked_reclaim_retains_claim_until_retry_can_remove_it` y
  `a_leftover_partial_fails_verification`.

### T9 — Rutas no Unicode irreabribles

- **Activo**: origen / cobertura.
- **Actor**: A1.
- **Precondición**: los nombres se guardan con `to_string_lossy`; la ruta
  exacta puede dejar de poder reabrirse.
- **Impacto**: archivos inventariados que no se pueden leer ni copiar; o peor,
  abrir un archivo **distinto** del inventariado.
- **Mitigación**: se conserva la representación raw UTF-16 exacta; las
  operaciones reales reconstruyen la ruta desde el raw, nunca desde el
  display.
- **Riesgo residual**: `display` sigue siendo lossy por definición; es solo
  para la UI.
- **Test**: `raw_path_round_trip`, `lossy_name_is_never_used_to_open`.

### T10 — Solapamiento de raíces por alias

- **Activo**: origen y salida.
- **Actor**: A1.
- **Precondición**: dos rutas distintas (junction, nombre 8.3 o escritura
  alternativa) pueden designar la misma carpeta aunque no se solapen
  lexicalmente.
- **Impacto**: la salida cae dentro del origen (o al revés) sin detectarse:
  el pipeline se autoalimenta o escribe en el origen.
- **Mitigación vigente**: además del filtro lexical, `df-fs-safety` resuelve el
  ancestro existente más profundo de cada raíz, canonicaliza aliases físicos y
  rechaza equivalencia o contención antes de crear la salida. Una raíz de
  origen que sea reparse point se rechaza antes de `read_dir`. Fachada y
  escáner comprueban origen/proyecto/salida/auditoría y pares de orígenes; el
  executor vuelve a comprobar origen/salida antes de escribir y valida las
  identidades congeladas en el manifiesto.
- **Riesgo residual aceptado**: permanece la carrera entre comprobación y uso
  frente a un proceso concurrente; las revalidaciones, fingerprints y la
  frontera no-replace la convierten en fallo en vez de autorizar una escritura
  silenciosa. NAS/UNC sigue experimental y puede ofrecer identidad degradada.
- **Tests**: `a_source_junction_is_rejected_and_cannot_hide_physical_overlap`,
  `create_rejects_a_source_junction_to_the_output_root`,
  `validation_rejects_a_source_root_junction_before_walking_it` y
  `a_source_root_repointed_to_output_is_rejected_before_any_write`.

### T11 — Fuente sustituida durante el chunking de similitud

- **Activo**: evidencia M0.3 y origen.
- **Actor**: A2.
- **Precondición**: un sincronizador sustituye o modifica el archivo después
  del hash canónico, antes o durante FastCDC.
- **Impacto**: chunks atribuidos al SHA-256 equivocado, relación falsa o mezcla
  de dos estados del archivo.
- **Mitigación**: solo se reabre la ruta raw; el fingerprint inventariado se
  compara antes de leer, vuelve a capturarse al terminar y el SHA-256 de todos
  los `ChunkData` debe coincidir con `content_objects`. Chunks, membresías,
  MinHash y bandas de un contenido están en una única transacción: cualquier
  error o caída la revierte completa.
- **Riesgo residual**: en filesystems sin identidad física, el fingerprint es
  degradado, pero el SHA-256 final todavía impide publicar bytes distintos.
- **Tests**: `dropped_content_writer_rolls_back_every_membership`,
  `synthetic_versions_are_related_but_never_identical` y las pruebas v2 de
  sustitución de `df-fs-safety`/`df-hash`.

### T12 — Explosión de pares o aproximación presentada como identidad

- **Activo**: disponibilidad, exactitud y control humano.
- **Actor**: A1 (corpus construido o accidental con chunks ubicuos).
- **Precondición**: muchos contenidos caen en la misma banda LSH o comparten un
  bloque común; un producto cartesiano agotaría RAM/disco. Alternativamente,
  una estimación MinHash alta podría presentarse como duplicado.
- **Impacto**: denegación de servicio, informe no exhaustivo oculto o una
  consolidación insegura.
- **Mitigación**: buckets con cardinalidad máxima, fallback solo para chunks
  poco frecuentes, candidatos en SQLite y techo estricto. Se sondea un par
  adicional para sellar `candidate_cap_reached` únicamente si hay cola
  truncada. Cada candidato se recalcula con multiconjunto/bytes exactos;
  SHA-256 sigue siendo la única identidad y la API de similitud no crea planes.
- **Riesgo residual**: los límites aceptan falsos negativos, visibles en CLI,
  estado, escritorio y ledger.
- **Tests**: `candidate_cap_is_exactly_signalled_and_persisted`,
  `completed_similarity_run_seals_candidates_relations_and_run` y
  `renders sealed M0.3 version evidence without implying an automatic action`.

## Qué sigue sin prometerse en M0.4

- Seguridad equivalente en Linux/macOS: **no implementada**; la ejecución se
  bloquea en plataformas sin primitivas seguras.
- NAS/UNC: experimental, sin pruebas suficientes.
- Durabilidad ante fallo físico del hardware.
- Protección frente a quien pueda modificar el binario o la base con
  privilegios: la garantía es de detección.

### T13 — Artefacto documental sustituido entre verificación y consumo

- **Activo**: índice Tantivy, snapshot Parquet y resultados derivados.
- **Actor**: A2/A3 con escritura concurrente en el directorio del proyecto.
- **Precondición**: sustituir archivo, directorio o junction después de su
  digest y antes de que Tantivy/DataFusion reabra la ruta.
- **Impacto**: consultar bytes no registrados, leer fuera del proyecto o
  atribuir resultados a un digest distinto.
- **Mitigación**: `ReadLease` abre sin seguir reparse points el objeto y todos
  sus ancestros. El digest usa el handle retenido; las reglas de sharing
  Windows impiden escritura, delete y rename hasta terminar la consulta. El
  build de Tantivy fija el directorio antes de entregarle la ruta; Parquet usa
  parcial reclamado y finalize por identidad/no-replace.
- **Riesgo residual**: añadir un archivo no referenciado después del recorrido
  no modifica los meta/segmentos ya bloqueados; el directorio completo se
  vuelve a derivar para cualquier build futuro. POSIX permanece fail-closed
  hasta M0.8.
- **Tests**: `read_lease_blocks_file_mutation_and_path_replacement`,
  `locked_directory_digest_covers_names_and_bytes` y
  `sql_is_read_only_bounded_and_integrity_checked`.

### T14 — Sidecar sustituido o secuestrado por el entorno

- **Activo**: frontera de aislamiento PDF/SQL.
- **Actor**: A2/A3.
- **Precondición**: `PATH`, variable de entorno, symlink o cambio del ejecutable
  redirige el launch.
- **Impacto**: código distinto procesa bytes no confiables con resultados
  atribuidos al motor.
- **Mitigación**: solo ruta absoluta explícita o hermano de `current_exe`; no
  `PATH`/env; archivo regular sin reparse, padre validado y lease vivo durante
  la ejecución; protocolo/version handshake; entorno del hijo vacío.
- **Riesgo residual**: la firma de los artefactos de release se cierra con la
  firma keyless de Sigstore (ADR-0039), que cubre checksums y SBOM; no hay
  firma Authenticode, así que quien sustituye el ejecutable antes de que el
  proceso padre lo arriende queda dentro del riesgo de cadena de suministro,
  no del parser.
