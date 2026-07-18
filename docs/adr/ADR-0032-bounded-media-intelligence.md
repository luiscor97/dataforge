# ADR-0032 — Inteligencia multimedia acotada y solo-revisión (M0.5)

**Estado:** Aceptada
**Fecha:** 2026-07-18
**Relacionada con:** RFC-0001 §21 y §45 M0.5; ADR-0029, ADR-0030, ADR-0031

## Contexto

M0.5 exige detectar *rediciones* del mismo material visual o acústico — la
foto recomprimida, la pista transcodificada, el vídeo reescalado — que la
identidad exacta SHA-256 no puede relacionar por diseño. El núcleo de motor
ya existía (`df-media`: pHash DCT de imagen, Chromaprint de audio, pHash por
keyframes de vídeo, workers aislados); faltaba convertirlo en un incremento
con persistencia, fachada, CLI y escritorio sin abrir ninguna vía a una
acción automática.

## Decisiones

1. **Runs direccionados por configuración, como M0.3.** Un run de medios se
   identifica por el SHA-256 de su configuración serializada (versión de
   contrato, versiones de los tres algoritmos, límites, techo de pares y las
   listas de extensiones por tipo). Cambiar cualquier parámetro crea otro
   run; repetir la misma configuración devuelve el run sellado sin releer el
   origen. La comparación de reutilización usa el texto crudo almacenado,
   nunca un round-trip JSON que reordene claves.

2. **Los sidecars son cableado de máquina, no evidencia.** Las rutas del
   worker de imagen y de FFmpeg no forman parte del digest: su ausencia
   produce filas `FAILED` con `WORKER_UNAVAILABLE` — evidencia explícita de
   lo que *no* se pudo mirar, jamás un hueco silencioso. El worker embebido
   se resuelve únicamente junto al ejecutable actual; FFmpeg solo por ruta
   absoluta explícita. Nunca `PATH` ni variables de entorno.

3. **Una analítica por contenido único, reanudable.** La selección pagina
   contenidos hasheados cuya extensión (en minúsculas, sin punto, como
   normaliza el escáner) pertenece a las listas del run. Cada contenido se
   reabre por su ocurrencia representativa estable, se verifica fingerprint
   antes de leer y SHA-256 completo tras leer; una fuente cambiada es un
   `Conflict` duro (rescan), no una evidencia degradada. Un corte deja el
   run `RUNNING` y la siguiente invocación continúa donde quedó.

4. **Comparación por pares acotada y determinista.** Solo dentro del mismo
   tipo, en orden estable de content id, con techo configurable de pares y
   sondeo de un par extra para que `pair_cap_reached` signifique cola real
   omitida. Las relaciones de un run interrumpido se reconstruyen
   (borrado permitido solo mientras `RUNNING`); tras el sellado, SQLite las
   rechaza por trigger.

5. **El esquema impide relaciones sin evidencia.** `media_relations` exige
   por trigger que ambos contenidos tengan filas `EXTRACTED` en el mismo
   run, par ordenado (`content_a < content_b`) y run `RUNNING`. El sellado
   valida que todos los contadores coincidan con las filas realmente
   escritas. `automatic_action: true` es irrepresentable: el contrato de
   dominio lo rechaza en deserialización.

6. **Tres relaciones, ninguna acción.** `IMAGE_PERCEPTUAL_MATCH`,
   `AUDIO_ACOUSTIC_MATCH` y `VIDEO_PERCEPTUAL_MATCH` con score en
   millonésimas y la evidencia de comparación literal del motor. CLI,
   fachada y escritorio las presentan como revisión; ningún camino las
   traduce a operaciones de plan.

## Alternativas consideradas

- **Comparar por pares entre todos los tipos** — descartado: sin semántica
  (un pHash de imagen no se compara con Chromaprint) y cuadráticamente caro.
- **Indexación LSH de fingerprints perceptuales** — pospuesto: los corpus
  multimedia reales del dominio (miles, no millones) no justifican aún la
  complejidad; el techo de pares hace el coste explícito y visible.
- **Incluir las rutas de sidecars en el digest** — descartado: convertiría
  un detalle de despliegue en identidad de evidencia y rompería la
  reutilización entre máquinas.

## Consecuencias

- La migración `0016_media_intelligence.sql` sigue la doctrina 0013:
  append-only, sellado validado por triggers y relaciones incapaces de
  autorizar operaciones.
- El fallo cerrado es visible: sin FFmpeg, audio y vídeo aparecen como
  `FAILED`/`WORKER_UNAVAILABLE` en la evidencia y en los contadores del
  run, nunca como "no había medios".
- Deuda aceptada: la selección es por extensión, no por sniffing de
  contenido; un `.dat` que sea un JPEG no se analiza en M0.5. El sniffing
  pertenece a la evolución del perfilado de contenido, con su propia
  versión de configuración.

## Tests

`tools/df-media-worker/tests/project_pipeline.rs` conduce el flujo real con
el worker aislado: dos rediciones JPEG del mismo material se relacionan y la
imagen ajena no; el run se sella, se reutiliza por digest y el ledger
verifica. La ausencia de workers produce evidencia explícita con el run
sellado igualmente. La UI tiene sus propios tests de estado pendiente y
evidencia sellada sin acción automática.
