# ADR-0036 — NAS endurecido: clasificación real y destino con identidad probada (M0.8)

**Estado:** Aceptada
**Fecha:** 2026-07-20
**Relacionada con:** RFC-0001 §45 M0.8; ADR-0019, ADR-0035

## Contexto

`FileSystemKind` existía desde M0.0 pero nadie lo rellenaba: todos los
roots quedaban `UNKNOWN` con la nota "la detección real pertenece a la
validación". Mientras tanto, media doctrina del motor depende de la
identidad física de NTFS/ReFS (ADR-0019): detección de sustitución,
leases de artefactos, reuso incremental. Escribir en un share de red o un
FAT sin decirlo es exactamente el hueco silencioso que el RFC prohíbe.

## Decisiones

1. **Clasificación real en la validación.** `classify_filesystem` en
   df-fs-safety: una ruta UNC (`\\server\...` o `\\?\UNC\...`) es
   `NETWORK` antes de tocar ninguna API; para rutas locales,
   `GetDriveTypeW` (remoto → `NETWORK`) y `GetVolumeInformationW` dan el
   nombre real (`NTFS`, `ReFS`, `FAT32`, `exFAT`; lo demás, `UNKNOWN`).
   En POSIX la clasificación es `UNKNOWN` hasta que exista el backend —
   y `UNKNOWN` se trata siempre como degradado, nunca como seguro.
   La validación persiste el resultado en `source_roots.filesystem`
   (metadato operativo del root, refrescable al re-validar) y el estado
   del proyecto lo muestra.

2. **Leer de NAS es caso de uso; escribir exige reconocimiento.**
   `FileSystemKind::has_physical_identity` es verdadero solo para
   NTFS/ReFS. El executor clasifica el output root al arrancar: sin
   identidad física y sin `--allow-degraded-destination`, la ejecución
   rechaza fail-closed con el motivo exacto. El flag es una decisión
   explícita por ejecución, no una configuración persistente.

3. **Los orígenes degradados no se bloquean, se registran.** El
   fingerprint por archivo ya degrada su garantía solo (ADR-0019, campos
   `none`), el reuso incremental ya se niega solo (ADR-0035) y el estado
   muestra el filesystem de cada root. Bloquear la lectura castigaría el
   caso de uso central sin ganancia de seguridad.

## Alternativas consideradas

- **Bloquear también orígenes de red** — descartado: el motor nunca
  escribe en el origen y sus garantías por archivo ya se degradan de
  forma visible.
- **Persistir el reconocimiento como configuración** — descartado: un
  "sí" antiguo no debe autorizar ejecuciones futuras; el coste de un flag
  por ejecución es deliberado.
- **Sniffing de capacidades (probar file ids en caliente)** — pospuesto:
  el nombre del filesystem más el tipo de unidad cubre los casos reales;
  una sonda activa pertenece al backend POSIX futuro.

## Consecuencias

- Ningún cambio para el flujo NTFS→NTFS habitual: la clasificación es
  una llamada por root en la validación y una por ejecución.
- En POSIX el executor ya rechazaba por plataforma antes de llegar a este
  gate; cuando exista escritura POSIX, este gate queda como segunda línea.
- Deuda declarada: sin prueba de integración con un share real (requiere
  red o loopback SMB con privilegios); la detección UNC y el gate tienen
  tests unitarios y el resto queda cubierto por la clasificación local.

## Tests

df-fs-safety: rutas UNC (planas y `\\?\UNC\`) clasifican `NETWORK` sin
tocar APIs; un directorio temporal local clasifica como volumen fijo con
nombre (NTFS en los runners). df-domain: tabla de `has_physical_identity`.
El gate del executor queda cubierto por su condición pura y el flujo NTFS
existente de los E2E.
