# ADR-0025 — Representante lógico de un conjunto de duplicados

**Estado:** Aceptada
**Fecha:** 2026-07-14
**Relacionada con:** RFC-0001 §15.5, §15.2, §18.3, §5.3, reglas 8 y 9;
ADR-0024

## Contexto

Milestone 0.1 ya detecta duplicados exactos (mismo tamaño, mismo SHA-256) y
los lista como evidencia, pero no dice *cuál* de las copias es la buena. El
§15.5 define un "representante lógico" con una puntuación configurable que
premia el contexto específico, el nombre limpio y la ruta canónica, y penaliza
Descargas, Escritorio, Backup, Copia, rutas injertadas y temporales. ADR-0024
acaba de aportar la mitad negativa de esa fórmula: las penalizaciones de
ubicación por carpeta (§18.3). Esta rebanada las usa para cerrar el criterio
"políticas de duplicado" de M0.2.

## Decisión(es)

1. **Se elige representante, no se propone borrar.** El §15.5 es explícito:
   "el representante lógico no implica borrar otras apariciones", y la regla 8
   prohíbe considerar un duplicado automáticamente prescindible. Se registra
   qué copia es la mejor ubicación canónica y por qué; no se genera ninguna
   operación de plan, no se marca nada para eliminación y el plan sigue
   copiando todas las apariciones (política `REPORT_ONLY`, §15.4).

2. **Coste determinista, menor es mejor.** Para cada aparición del conjunto:

   ```text
   coste = penalización_ubicación * 100
         + (10 si el nombre tiene marca de copia)
         + profundidad_de_ruta
   score = -coste
   ```

   La penalización de ubicación es la **peor** de todas las carpetas
   ancestras de la aparición (`folder_contexts`, §18.3): un archivo dentro de
   `Backup/algo/…` hereda la penalización de `Backup`. Los pesos hacen que la
   ubicación domine sobre el nombre y este sobre la profundidad, que solo
   actúa como desempate fino a favor de la ruta más canónica.

3. **Desempate estable.** A igual coste gana la ruta absoluta menor
   lexicográficamente, de modo que el resultado es reproducible entre
   ejecuciones y máquinas.

4. **Señales del §15.5 implementadas y aplazadas.** Implementadas:
   `- Descargas/Escritorio/Backup/Copia/temporal` (vía penalización de
   ubicación), `+ nombre limpio` (marcas `- copia`, `copia de …`, `nombre (1)`)
   y `+ ruta canónica` (profundidad). Aplazadas por falta de señal:
   `+ contexto específico` y `+ fecha coherente` requieren el grafo de
   entidades (§18.2); `+ menor anomalía` requiere el detector de anomalías; y
   `- ruta injertada` requiere las relaciones de árbol parciales/embebidas de
   ADR-0023. Ninguna se simula.

5. **Evidencia por decisión (§5.3).** Junto al representante se guarda un
   `reason` legible ("outside any generic folder; clean file name; path depth
   1") que explica la elección. Es el criterio "evidencia por decisión" de
   M0.2 y hace la decisión auditable sin releer el código.

6. **Dónde se ejecuta y persistencia.** Corre dentro de `analyze`, **después**
   de la clasificación de contexto (necesita sus penalizaciones), en la
   transición `HASHED → ANALYZING → ANALYZED`. Se persiste en
   `duplicate_representatives` (migración `0006_representatives.sql`), una
   fila por conjunto. Idempotente: se reemplazan las filas del snapshot. Emite
   `DUPLICATE_REPRESENTATIVES_SCORED`. El informe `report duplicates` marca la
   copia elegida con `*` y muestra su razón.

## Alternativas consideradas

- **Elegir por fecha más antigua o más reciente** — descartada: la fecha del
  sistema de archivos es poco fiable en material copiado entre discos y
  backups (justo el caso de uso), y el §15.5 la lista solo como "fecha
  coherente", una señal débil que además necesita contexto para interpretarse.
- **Elegir la ruta más corta sin más** — descartada: premiaría una copia
  suelta en la raíz de Descargas frente al archivo bien archivado dentro de un
  expediente; por eso la ubicación pesa 100× más que la profundidad.
- **Marcar las no representantes como prescindibles / generar operaciones de
  consolidación** — descartada: viola el §15.5 y la regla 8. La consolidación
  necesita además política de duplicados por perfil (§15.4) y contextos
  protegidos, que aún no existen.
- **Guardar el score de todas las apariciones, no solo del ganador** —
  descartada de momento por coste de almacenamiento sin uso actual; el
  `reason` del ganador basta para explicar la decisión. Se reconsiderará si la
  UI de revisión (M0.2) necesita mostrar el ranking completo.

## Consecuencias

- Cierra el criterio "políticas de duplicado" de M0.2 en su versión segura:
  DataForge ya sabe *qué copia recomendaría* y lo justifica, sin tocar nada.
- La calidad de la elección depende directamente de ADR-0024: si una carpeta
  genérica no está en la tabla de marcadores, su contenido no se penaliza.
  Ampliar marcadores mejora esta decisión sin cambiar este algoritmo.
- Deuda aceptada: las señales aplazadas del punto 4; la consolidación guiada
  por representante (necesita §15.4 y perfiles); y el ranking completo para la
  UI de revisión.
- Condición de revisión: cuando existan perfiles con contextos protegidos, el
  representante debe respetar que una copia dentro de una frontera protegida
  no compite con otra fuera de ella (regla 9); revisar entonces la fórmula.
