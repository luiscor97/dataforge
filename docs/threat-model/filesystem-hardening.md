# Modelo de amenazas — Filesystem Safety Hardening (v0.1.1-dev)

Ámbito: el pipeline `scan → hash → analyze → plan → approve → execute →
verify` frente a un sistema de archivos **hostil o simplemente tramposo**.
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
- **Mitigación**: parciales con nombre propio reconocible y limpiables solo
  por DataForge; reanudación que re-ejecuta `RUNNING`; el verificador reporta
  parciales huérfanos.
- **Riesgo residual**: ante fallo físico del disco no hay garantía absoluta de
  durabilidad; se documenta qué ofrece NTFS y qué no (ADR-0021).
- **Test**: `crash_with_partial_file_resumes`, `orphan_partial_is_a_finding`.

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
- **Precondición**: las comprobaciones de solapamiento son **lexicales**. Dos
  rutas distintas (junction, 8.3, UNC, mayúsculas) pueden ser la misma
  carpeta.
- **Impacto**: la salida cae dentro del origen (o al revés) sin detectarse:
  el pipeline se autoalimenta o escribe en el origen.
- **Mitigación**: servicio `PathRelation` que resuelve por identidad física
  cuando puede y devuelve `Unresolved` (conservador) cuando no.
- **Riesgo residual**: NAS/UNC **no** se anuncia como plenamente soportado
  hasta tener pruebas.
- **Test**: `two_representations_of_same_folder_are_detected`.

## Qué NO se promete en v0.1.1-dev

- Seguridad equivalente en Linux/macOS: **no implementada**; la ejecución se
  bloquea en plataformas sin primitivas seguras.
- NAS/UNC: experimental, sin pruebas suficientes.
- Durabilidad ante fallo físico del hardware.
- Protección frente a quien pueda modificar el binario o la base con
  privilegios: la garantía es de detección.
