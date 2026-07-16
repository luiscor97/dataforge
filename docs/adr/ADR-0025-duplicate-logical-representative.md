# ADR-0025 — Representante lógico de un conjunto de duplicados

**Estado:** Aceptada
**Fecha:** 2026-07-14
**Relacionada con:** RFC-0001 §15.5, §15.2, §18.3, §5.3, reglas 8 y 9;
ADR-0024, ADR-0026

**Revisada:** 2026-07-16 para separar la elección del representante de la
autorización explícita de una política de consolidación.

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
   operación de plan y no se marca nada para eliminación. `REPORT_ONLY`, la
   política por defecto, sigue copiando todas las apariciones. Una política
   opt-in sí puede emitir `SKIP_REPRESENTED`, pero solo después de clasificar el
   conjunto y nunca para una aparición en frontera protegida (ADR-0026).

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
   lexicográficamente. El resultado es reproducible para el mismo inventario y
   las mismas rutas de raíces; no se promete que mover las raíces a nombres
   distintos en otra máquina conserve el mismo ganador.

4. **Señales del §15.5 implementadas y aplazadas.** Implementadas:
   `- Descargas/Escritorio/Backup/Copia/temporal` (vía penalización de
   ubicación), `+ nombre limpio` (marcas `- copia`, `copia de …`, `nombre (1)`)
   y `+ ruta canónica` (profundidad). Aplazadas por falta de señal:
   `+ contexto específico` y `+ fecha coherente` carecen de señales
   estructuradas; `+ menor anomalía` no forma parte de la fórmula aunque M0.2
   ya materialice anomalías; y `- ruta injertada` tampoco se incorpora al
   score aunque ADR-0027 detecte relaciones parciales y embebidas. Ninguna de
   esas señales se simula dentro del ranking.

5. **Evidencia por decisión (§5.3).** Junto al representante se guarda un
   `reason` legible ("outside any generic folder; clean file name; path depth
   1") que explica la elección. Es el criterio "evidencia por decisión" de
   M0.2 y hace la decisión auditable sin releer el código.

6. **Dónde se ejecuta y persistencia.** Corre dentro de `analyze`, **después**
   de la clasificación de contexto (necesita sus penalizaciones), en la
   transición `HASHED → ANALYZING → ANALYZED`. Se persiste en
   `duplicate_representatives` (migración `0008_representatives.sql`), una
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
- **Tratar toda no representante como prescindible** — descartado: viola el
  §15.5 y la regla 8. Las políticas implementadas distinguen contexto,
  preservan incertidumbre y hacen prevalecer siempre las fronteras
  protegidas. El representante por sí solo no autoriza nada.
- **Guardar el score de todas las apariciones, no solo del ganador** —
  descartada de momento por coste de almacenamiento sin uso actual; el
  `reason` del ganador basta para explicar la decisión. Se reconsiderará si
  una interfaz necesita mostrar el ranking completo.

## Consecuencias

- DataForge sabe qué copia usar como representante y lo justifica. La decisión
  de copiar todas o representar algunas apariciones permanece separada y
  explícita en la política del plan.
- La calidad de la elección depende directamente de ADR-0024: si una carpeta
  genérica no está en la tabla de marcadores, su contenido no se penaliza.
  Ampliar marcadores mejora esta decisión sin cambiar este algoritmo.
- Las políticas `CONSOLIDATE_WITHIN_CONTEXT`,
  `CONSOLIDATE_GENERIC_COPIES` y `CONSOLIDATE_ALL` usan el representante, pero
  conservan contextos desconocidos y protegidos. `REPORT_ONLY` y
  `PRESERVE_ALL` nunca consolidan.
- Deuda aceptada: las señales aplazadas del punto 4 y el ranking completo de
  candidatas para revisión.
- Condición de revisión: incorporar una señal nueva exige explicar su peso y
  demostrar que no permite omitir una aparición protegida o incierta.
