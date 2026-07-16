# Modelo de amenazas del núcleo local

Ámbito: fundación, pipeline seguro y cierre de Milestone 0.2 (objetivo
`0.2.0`). La lista objetivo completa del producto está en RFC-0001 §37.

> Junctions, symlinks, carreras TOCTOU, sustitución del origen, finalize y
> manifiesto de ejecución se desarrollan en
> [`filesystem-hardening.md`](filesystem-hardening.md). Este documento cubre
> además la evidencia estructural, perfiles, reglas, revisión y recuperación.

## Modelo de atacante

- **Contenido heredado no confiable:** nombres engañosos, rutas extremas,
  reparse points, errores de lectura y ramas parcialmente recuperadas.
- **Interrupción o proceso concurrente:** cierre forzado, corte de energía,
  sincronizador o usuario que cambia archivos entre fases.
- **Manipulador local del proyecto:** puede editar el marker o SQLite, pero no
  el binario en ejecución ni la clave de confianza del sistema operativo.
- **Error de configuración:** typo de perfil, política distinta al reintentar o
  regla declarativa inválida.

No se pretende resistir a un administrador capaz de reemplazar el binario o
modificar arbitrariamente proceso, base y salida a la vez.

## Activos

1. Los archivos de origen y su identidad exacta.
2. La salida: confinamiento, ausencia de sobrescritura e integridad.
3. El estado del proyecto (`state/dataforge.sqlite`).
4. El contrato aprobado (`execution_manifest` + SHA-256 del plan).
5. La evidencia estructural: relaciones, anomalías y marcador de completitud.
6. El historial humano: cola, decisiones y justificaciones de revisión.
7. El ledger append-only y la cadena de suministro del repositorio.

## Amenazas y mitigaciones vigentes

| Amenaza | Mitigación implementada | Riesgo residual |
| --- | --- | --- |
| El motor escribe o borra en el origen | Orígenes de solo lectura por constructor y `CHECK`; no existe ruta de borrado del origen; toda escritura de salida pasa por `df-fs-safety`; pruebas de origen intacto | Un proceso ajeno puede cambiar el origen; fingerprints y hashes lo detectan según capacidad del filesystem |
| Una ruta redirige la escritura o sobrescribe un destino | Resolución segura de componentes, rechazo de reparse points y finalize no-replace; verificación que no sigue enlaces | Windows es la plataforma endurecida; NAS/UNC mantiene identidad degradada. Detalle en el modelo de filesystem |
| Se ejecuta algo distinto de lo aprobado | Manifiesto completo e inmutable, SHA-256 canónico, triggers y ejecución exclusiva desde el manifiesto; verificación recalcula el hash | Quien reescriba coherentemente toda la base o controle también el binario queda fuera del modelo |
| Manipulación o truncado del ledger | Cadena SHA-256 con envelope canónico, secuencia contigua, triggers append-only y verificación criptográfica | Al no existir firma externa ni secreto, un atacante con control total offline puede reescribir la base y recalcular la cadena |
| Deriva silenciosa del esquema | Checksum SHA-256 de cada migración en apertura; integridad y claves foráneas | Una migración publicada no puede corregirse in place: requiere otra migración |
| Un análisis interrumpido aparece como informe vacío válido | `analysis_completions` append-only por snapshot, evento final único y guarda de estado estable; ningún informe estructural responde solo porque exista una tabla parcial | Un atacante con control total de SQLite puede falsificar marcador y estado; no hay anclaje externo de la base |
| Un reintento crea otra versión de plan o duplica aprobación/manifiesto | Recuperación explícita desde `ANALYZING`, `PLANNING` y `PLAN_REVIEW`; reutilización del plan `READY` tras comparar operaciones; verificación de operaciones, manifiesto y hash ya aprobados (ADR-0029) | No es un protocolo multiwriter distribuido; se asume un proyecto SQLite local |
| Un typo de perfil elimina fronteras jurídicas | Los ids desconocidos se rechazan al crear, abrir y analizar; `generic` solo se usa cuando se selecciona o se deja como valor por defecto | Los marcadores por nombre pueden no reconocer una frontera con nombre no declarado |
| Una regla declarativa adquiere capacidad destructiva o escapa por la ruta | Perfiles embebidos y validados; glob únicamente sobre nombre sin separadores; acciones cerradas a cuatro operaciones de copia; frontera protegida prevalece | Una regla puede clasificar de más o de menos; el efecto es una copia en otra categoría o revisión, no borrado |
| Una anomalía ambigua se resuelve automáticamente perdiendo una copia | Hallazgos con evidencia canónica, `review_items` estables y decisiones append-only con justificación; pendientes generan `COPY_REVIEW`; reglas/revisión conservadoras prevalecen sobre deduplicación | El usuario puede tomar una decisión equivocada, pero queda registrada y sigue sin modificar el origen |
| Las relaciones de árboles pierden contenido exclusivo | Solo ramas completas; contenidos exactos; recuentos exclusivos A/B persistidos; parciales y embebidas son evidencia/revisión, nunca permiso automático de consolidación | No se persiste la lista completa de rutas exclusivas y los límites pueden omitir pares |
| Explosión cuadrática o selección no reproducible de pares | Índice invertido, mínimo de dos contenidos, exclusión de componentes en más de 32 carpetas, techo de 200 000 pares y orden/orientación por `(source_root_id, relative_path)`, no por UUID de carpeta (ADR-0027) | Los límites introducen falsos negativos deliberados |
| La UI aplica lógica privilegiada | CLI y desktop consumen `df-facade`; la UI presenta DTOs y no abre SQLite; capacidades Tauri y CSP acotadas | Un bug de presentación puede confundir, pero no salta las validaciones del motor |
| Dependencia o bootstrap comprometidos | Lockfiles, `cargo audit`, `cargo deny`, fuentes/licencias acotadas, CI y prohibición de `irm | iex` | Riesgo normal de cadena de suministro; firma de releases y SBOM siguen pendientes |

## Propiedades de fallo cerrado

- Perfil explícito desconocido: error, no fallback.
- Snapshot sin marcador final: informe rechazado, aunque haya resultados
  parciales.
- Política distinta al recuperar un plan persistido: conflicto, no nueva
  versión silenciosa.
- Manifiesto o SHA-256 incoherente al recuperar aprobación: conflicto, no
  regeneración.
- Regla inválida o acción fuera del conjunto seguro: perfil rechazado.
- Contexto desconocido o protegido: copia conservada bajo cualquier política.

## Riesgos aceptados y límites

- La clasificación de carpetas y las reglas se basan en nombres. No interpretan
  el contenido de un documento ni demuestran que dos carpetas pertenezcan al
  mismo asunto.
- El análisis estructural usa identidades exactas de contenido y límites de
  candidatos. Puede omitir relaciones y no consolida árboles automáticamente.
- La revisión humana es auditable, no infalible. Una decisión incorrecta puede
  organizar mal la salida, aunque el origen permanece intacto.
- El marker del proyecto no está firmado y no es fuente de verdad. Sus campos
  se validan y no pueden redirigir la base fuera del proyecto.
- La seguridad equivalente en Linux/macOS, NAS/UNC plenamente validado,
  durabilidad ante fallo físico, sandboxing de plugins, firma de releases y
  SBOM permanecen fuera del alcance de `0.2.0`.
