# Skill: dataforge-invariants

**Nombre:** dataforge-invariants
**Objetivo:** lista de control de invariantes que ningún cambio puede violar;
revisión previa a cualquier merge.

## Cuándo usarla

- Al revisar o autor(izar) cualquier PR.
- Antes de implementar una funcionalidad nueva, para diseñar dentro de las
  reglas.

## Entradas

- El diff completo del cambio propuesto.

## Salidas

- Veredicto: cumple / viola (con la regla concreta y la línea).

## Lista de control

1. **Origen inmutable** — ¿alguna ruta de código abre archivos de origen con
   escritura, o crea/borra/renombra dentro de un source root?
2. **Sin borrado / sin sobrescritura** — ¿aparece `remove_file`, `remove_dir`,
   escritura sobre destino existente, o un flag "force"?
3. **SQLite única fuente de verdad** — ¿se está guardando estado en JSON,
   memoria global o archivos sueltos en lugar de `df-db`?
4. **Estados solo por máquina de estados** — ¿algo asigna `project.state`
   directamente o hace UPDATE de `state` fuera de `update_project_state`?
5. **Ledger completo** — ¿toda mutación emite su evento en la MISMA
   transacción? ¿algún código toca `audit_events` fuera de `append_event`?
6. **Clientes solo vía df-facade** — ¿CLI o UI importan `df-db`/`rusqlite` o
   hacen I/O de proyecto por su cuenta?
7. **Sin adelantar milestones** — ¿introduce escáner real, hashing, FastCDC,
   búsqueda, plugins o IA antes de su hito?
8. **Sin funcionalidad falsa** — ¿botones sin efecto, estados simulados,
   datos mock presentados como reales?
9. **Evidencia de pruebas** — ¿el cambio trae tests y la puerta de calidad
   ejecutada?

## Límites

- Esta skill no aprueba excepciones: una violación necesita ADR o RFC según
  RFC-0001 §50, no una justificación en el PR.

## Criterios de éxito

- Los 9 puntos contestados explícitamente en la revisión.

## Fallos esperados

- Falsos positivos en tests (los tests sí pueden borrar sus directorios
  temporales propios; el límite aplica a datos de usuario/proyecto).
