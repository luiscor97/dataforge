# ADR-0044 — `df-agent`: el bucle de reconstrucción autónoma

**Estado:** Propuesta
**Fecha:** 2026-07-21
**Relacionada con:** RFC-0002 (`df-agent`); ADR-0043 (superficie de
herramientas), ADR-0041 (`df-rules`), ADR-0042 (consentimiento por política);
ADR-0029 (reanudación), ADR-0018 (manifiesto de ejecución inmutable)

> Numeración reservada por RFC-0002; el número final se fija al commitear.

## Contexto

Con la superficie de herramientas (0042), el motor de reglas (0040) y el
consentimiento por política (0041), falta el **orquestador** que ejecute el
algoritmo real de dos pasadas de extremo a extremo: desatendido, reanudable y
**sin detenerse nunca** ante una duda (requisito central del RFC-0002, nacido de
que un run corre en discos viejos durante horas o días).

## Decisión

**Crate nuevo `df-agent`: un bucle que conduce la fachada por sus herramientas y
delega toda autoridad en `df-rules`.** El ciclo de vida:

1. **Intención (Fase A).** Captura el objetivo en lenguaje natural, el dial
   **conservador/agresivo** y (opcional) una confirmación humana única de "qué
   entendí" antes del run largo.
2. **Inventario.** `scan` + `hash`, **aislando media primero** (como el trabajo
   real): la media no entra en el razonamiento documental.
3. **Pasada 1 — dedup + representante.** Agrupa por SHA-256; `df-rules` puntúa y
   elige representante. Resuelve el grueso **sin IA**.
4. **Pasada 2 — árboles injertados.** Clasifica los injertos por estado;
   `df-rules` enruta a **reemplazo limpio** (auto) o **contenido único en
   contexto sospechoso** (`revisar/`).
5. **Diagnóstico de modo.** Si hay árbol canónico dominante → Modo 1 (recuperar).
   Para lo suelto sin encaje → Modo 2 (la IA **inventa taxonomía**, bajo
   consentimiento por política) y lo documenta.
6. **Ensamblar el plan completo** (cada archivo → destino o `revisar/<ruta>` +
   motivo) y **congelarlo** (manifiesto 0004, triggers de inmutabilidad).
7. **Dry-run opcional:** emite el plan + qué reglas dispararían, sin copiar.
8. **Ejecutar** (copia-solo) → **verificar** (re-lectura y re-hash independientes)
   → **informe** (esquema de organización + mapa origen→destino).

Garantías del bucle:

- **No bloquea jamás.** Duda, ambigüedad o frontera dura → `revisar/` (árbol
  espejo, mejor apuesta) y sigue.
- **Reanudable** desde el manifiesto congelado + el ledger de operaciones
  (ADR-0029); lo ya colocado (identidad física, fingerprint v2) es no-op.
- **Robustez de disco viejo:** error de lectura → `revisar/_ilegible/`;
  verificación fallida → `revisar/_verificacion-fallida/`; **pre-vuelo de
  espacio** antes de copiar; el run nunca aborta por un archivo.
- **Presupuestos y cortacircuitos:** tokens/divulgación/tiempo/operaciones; "para
  y manda a `revisar/` en bloque si la tasa de ambigüedad supera X%".
- **La autoridad es `df-rules`, no el modelo.** `df-agent` propone y ejecuta; el
  veredicto de aprobación y su procedencia los sella el gate autónomo.

## Alternativas consideradas

- **Escalado bloqueante a humano durante el run** — descartado: detiene el trabajo
  horas/días; la ambigüedad se resuelve con `revisar/`, no bloqueando.
- **Agente con autoridad directa sobre el gate** — descartado: la autoridad es la
  regla determinista, no la IA.
- **Orquestador monolítico** — descartado: reutiliza las operaciones de fachada
  (scan/analyze/plan/approve/execute/verify) que ya existen y ya están probadas.

## Consecuencias

- Un caos de disco se reconstruye **desatendido y hasta el final**, con todo
  reversible (copia-solo, origen intacto) y auditable (mapa origen→destino).
- **Deuda**: (a) detección de clones **parciales** (ADR-0023) que alimenta el
  `→ revisar`; (b) UI de escritorio para intención, progreso y cola de `revisar/`.
- **Revisar si**: se quiere un modo interactivo (copiloto L1) en la misma UI — es
  el mismo bucle con el humano en el gate en vez de `df-rules`.
