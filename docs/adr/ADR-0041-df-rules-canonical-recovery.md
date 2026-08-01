# ADR-0041 — Motor de reglas `df-rules`: recuperación canónica

**Estado:** Propuesta
**Fecha:** 2026-07-21
**Relacionada con:** RFC-0002 (Modo 1, `df-rules`); RFC-0001 §25 (reglas
declarativas); ADR-0023 (tree-clones), ADR-0024/0025 (contexto y representante),
ADR-0043 (superficie de herramientas)

> Numeración reservada por RFC-0002; el número final se fija al commitear, para
> no colisionar con el trabajo en paralelo de M1.0.1.

## Contexto

El algoritmo de recuperación canónica no es teórico: existe, se ejecutó sobre
156.962 archivos reales y sus artefactos son la fuente de verdad
(`05_Asesoria_Juridica_v2\Nueva carpeta\`: `revision_documental.csv` con la
fórmula en `auto_reason`, `estructura\resumen_estructura.md`, los buckets de
`03_Revision_Estructural`). Su **forma** es:

- un **score ponderado** para elegir representante entre duplicados exactos;
- una **clasificación de estado** de los árboles injertados;
- un **enrutado** auto vs. revisión;
- unas pocas **guardas categóricas** que no admiten excepción.

`df-rules` debe codificar eso sin (a) convertir la seguridad en confianza en el
modelo, ni (b) fijar en código pesos que el usuario debería poder afinar por
corpus o dominio.

## Decisión

**Motor de evaluación en Rust; parámetros afinables en tabla SQLite versionada.**

1. **Evaluación determinista en Rust.** `df-rules` recibe un plan propuesto y
   devuelve `Autorizar | A-revisar | Denegar` **con el id del conjunto de reglas
   y el score** que lo determinó. Nada de esto lo decide el modelo de IA: la IA
   propone, `df-rules` verifica sobre evidencia recalculada localmente.

2. **Parámetros como datos versionados y con checksum** (misma disciplina que las
   migraciones; deriva rechazada al abrir). Afinables sin recompilar:
   - **Score de representante** (duplicados exactos, de la fórmula real):
     `score = −8·profundidad − 1,1·longitud_ruta + 15·(mtime más antiguo)`.
     Gana el más superficial, de ruta más corta y **mtime más antiguo = original**
     (la copia recibe fecha nueva al copiarse). Los tres pesos son parámetros.
   - **Umbrales de auto-aprobación** (margen de score / confianza mínima).
   - **Penalizaciones de contexto** (`Backup/Copia/Temp/Recuperado`, ADR-0024/0025).

3. **Enrutado por estado de injerto** (de `resumen_estructura.md`):
   - `mirror_exact_by_path` / `mirror_or_duplicate_near_complete` → réplica (casi)
     completa → **colapsable** (Autorizar).
   - `mixed_duplicate_and_unique_review` → clon **parcial** → **A-revisar**.
   - **Reemplazo limpio** (misma SHA-256 en ruta más limpia) → **Autorizar**
     (contenido idéntico probado, riesgo nulo).
   - **Contenido único en contexto sospechoso** (sin duplicado, en árbol injertado)
     → **A-revisar**.

4. **Fronteras duras en código** (invariantes, no parámetros; fail-closed):
   - **Nunca deduplicar por nombre**: mismo nombre + hash distinto = documentos
     distintos.
   - **Evidencia compartida entre asuntos** (misma imagen/hash en varios
     expedientes) → siempre `A-revisar`.
   - **Destino vacío obligatorio**; **nunca borrar ni sobrescribir** (invariante
     del motor).
   - **Perfil de dominio protegido** (p. ej. jurídico) → `A-revisar` por defecto,
     nunca auto-colapsar.

5. **Procedencia sellada.** Cada veredicto autónomo registra en el ledger
   (RFC-0002): id + digest del conjunto de reglas, score, estado de injerto,
   evidencia. "¿Por qué se autorizó/derivó esto?" tiene respuesta determinista.

## Alternativas consideradas

- **DSL declarativo propio** — descartado: superficie de parser y de seguridad
  innecesaria para lo que es scoring ponderado + umbrales + guardas.
- **Todo hardcoded en Rust** — descartado: los pesos y umbrales deben afinarse por
  corpus/dominio sin recompilar; un valor equivocado no debería exigir release.
- **Todo como datos en SQLite** — descartado: las fronteras duras son invariantes
  y deben vivir en código testeado, no como filas editables.

## Consecuencias

- Pesos y umbrales **afinables y auditables**; deriva del conjunto detectada por
  checksum al abrir (como las migraciones).
- Habilita un **modo sombra**: correr `df-rules` registrando qué habría
  autorizado vs. la decisión humana, para calibrar umbrales sobre el corpus real
  antes de confiarle la auto-aprobación.
- **Deuda**: la detección de clones **parciales** (`mixed_duplicate_and_unique_
  review`) es la deuda abierta de ADR-0023; `df-rules` la consume pero su cómputo
  vive en el análisis estructural.
- **Revisar si**: aparece una clase de regla que no sea expresable como
  score+umbral+guarda (entonces reconsiderar un DSL acotado).
