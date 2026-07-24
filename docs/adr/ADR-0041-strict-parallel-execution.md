# ADR-0041 — Ejecución estricta paralela (M1.0.1)

- Estado: Aceptada (opt-in; no predeterminado hasta cumplir §9 del diseño)
- Fecha: 2026-07-22
- Contexto: RFC-0001 §27; `docs/performance/strict-parallel-execution-design.md`,
  `docs/performance/m1.0.1-baseline.md`, `docs/performance/m1.0.1-results.md`

## Contexto

La baseline mide que la ejecución domina el pipeline (81 %) y que el coste es
latencia por archivo —commits SQLite y syscalls—, no ancho de banda: copiar
los bytes es el 5,7 %. El diseño (revisado y juzgado sólido) construye la
paralelización desde el modelo de recuperación por operación, con todas las
ventanas de caída documentadas.

## Decisión

1. **Un solo coordinador dueño de SQLite.** Lease, claim de identidad y
   registro del resultado los hace el coordinador; los workers **nunca** abren
   la base (regla 15), extendiendo a `execute` lo que ya cumplían hash/verify.
2. **Costura prepare/claim/finish.** `copy_file` se parte en `prepare_copy`
   (sin SQLite: valida origen, reserva destino, crea el parcial y captura su
   identidad), la **barrera de claim** (único toque de SQLite del protocolo por
   archivo) y `finish_copy` (sin SQLite: copia, hashes, `sync_all`,
   revalidación, finalize no-replace). El worker cruza la barrera enviando la
   identidad al coordinador y esperando el commit durable **antes** de copiar,
   igual que el secuencial lo commitea en línea.
3. **Directorios primero + exclusión por destino.** Un pre-stage secuencial
   crea todos los directorios antes de la fase paralela; un `DestinationGuard`
   difiere ops con destino en conflicto. El finalize no-replace de plataforma
   convierte cualquier carrera residual en un reintento seguro, nunca una
   corrupción.
4. **Paginación por tipo.** El coordinador pagina copies y directorios por
   separado (`executable_copy_operations`/`executable_directory_operations`):
   una ventana `ORDER BY sequence LIMIT n` sobre tipos mezclados puede llenarse
   del otro tipo y ocultar trabajo pendiente. (Bug real medido y arreglado; el
   camino secuencial conserva la query mixta, que es segura porque completa
   cada op antes del siguiente fetch.)
5. **Opt-in hasta demostrarlo.** El default de `execute` sigue **secuencial**;
   el paralelo se activa con `--workers`. A diferencia de hash/verify (ya
   probados y por defecto), `execute` paralelo espera la aceptación completa de
   inyección de fallo (§9 del diseño, Increment 5) antes de ser el default.

## Consecuencias

- Ganancia medida ~2,0× en archivos pequeños (satura en 4 workers): el techo
  son los tres commits SQLite por operación, serializados por el coordinador.
  Superarlo es agrupar commits en microlotes (Increment 4), no más hilos.
- La recuperación por operación no cambia: `workers=1` reproduce el secuencial
  byte a byte (probado), y la cancelación solo deja de tomar trabajo nuevo.
- Falta, antes de mover el default: los tests de inyección de fallo por ventana
  A–F y respuestas tardías/duplicadas/en pánico bajo el pool (Increment 5), y
  los perfiles de durabilidad (Increment 6).
