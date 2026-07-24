# ADR-0040 — Hash y verificación paralelos acotados (M1.0.1)

- Estado: Aceptada
- Fecha: 2026-07-21
- Contexto: RFC-0001 §12.3/§14/§28; `docs/performance/m1.0.1-baseline.md`,
  `docs/performance/m1.0.1-results.md`

## Contexto

La baseline medida muestra que la ejecución domina el pipeline y que el
coste está en la latencia por archivo (commits SQLite y syscalls), no en el
ancho de banda. El hashing y la verificación re-leen y re-hashean cada
archivo de forma independiente: son candidatos naturales a paralelizar sin
tocar el protocolo de seguridad, siempre que la base siga con un único
escritor y el resultado sea idéntico al secuencial.

## Decisión

1. **Coordinador de base único.** `hash_project` y `verify_project`
   conservan un solo hilo que habla con SQLite: obtiene el lote, entrega
   trabajos inmutables a un pool acotado y persiste el resultado. Los
   workers **nunca** abren la base (regla 15).
2. **Pool acotado con `std::thread::scope`** (sin dependencias nuevas). Cada
   worker tiene su propio buffer reutilizable y toma trabajos por un índice
   atómico, de modo que la carga se autoequilibra ante tamaños de archivo
   desiguales.
3. **`--workers auto|N`.** `auto` limita a `min(paralelismo del equipo, 8)`:
   usar todos los hilos lógicos en E/S mixta rara vez es lo más rápido y hay
   que medirlo, no suponerlo. `workers=1` reproduce el camino secuencial.
4. **Determinismo como invariante.** Los resultados se identifican por
   trabajo, no por orden de llegada; el hashing se identifica por contenido y
   la verificación ordena sus hallazgos de forma canónica (severidad, tipo,
   ruta, detalle) y su veredicto es por conteo. `workers=1` y `workers=N`
   producen salida byte-idéntica, con tests que lo fijan.
5. **Recuperación intacta.** La cola persistente del hash y la lectura desde
   evidencia primaria de la verificación no cambian: la cancelación solo deja
   de tomar trabajos nuevos (los ya empezados terminan; los no empezados
   quedan pendientes para reanudar).

## Consecuencias

- Ganancia proporcional al trabajo por archivo: ~2,5× en archivos grandes
  (limitado por el ancho de banda del NVMe), ~1,26× en archivos pequeños
  (limitado por latencia de syscalls y por el coordinador serial). Las cifras
  y su causa están en `m1.0.1-results.md`; no se maquillan.
- La ejecución **no** se paraleliza aquí. Hacerlo exige mover la escritura de
  SQLite fuera del hilo de trabajo preservando la recuperación por operación;
  el diseño está en `strict-parallel-execution-design.md` y queda como
  propuesta a revisar antes de implementar.
- Ningún buffer es proporcional al número de archivos: uno por worker (≤ 8).
- Las opciones `workers` de `HashOptions`/`VerifyOptions` son ajustes de
  ejecución, no contratos congelados; su valor por defecto (`auto`) no cambia
  ningún resultado persistido.
