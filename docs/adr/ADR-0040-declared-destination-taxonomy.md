# ADR-0040 — Taxonomía de destino declarada por el perfil (2.0)

**Estado:** Aceptada (subsumida en RFC-0002 como mecanismo del paso 1)
**Fecha:** 2026-07-29 (coordinación resuelta 2026-08-01)
**Relacionada con:** RFC-0001 §26, §15.4; RFC-0002 §«El mecanismo: raíces
declaradas»; ADR-0019, ADR-0026, ADR-0037

> **Coordinación con RFC-0002: resuelta.** Esta ADR se escribió sin conocer el
> borrador de RFC-0002 (2026-07-21), y durante un tiempo pareció que las dos
> competían por la misma pregunta. Leídas enteras, no: **RFC-0002 fija la
> política** —qué forma tiene la salida— y **esta ADR el mecanismo** —cómo se
> declara el conjunto de raíces que la componen. RFC-0002 no especifica el
> mecanismo y lo necesita, porque su espejo de `revisar/` tiene que ser él mismo
> una raíz declarada y sus nombres tienen que quedar reservados frente a los
> orígenes.
>
> Por eso esta ADR **se subsume en RFC-0002 como el mecanismo de su paso 1**, y
> no se mantiene aparte. Las decisiones 1, 2, 3, 4 y 6 se conservan íntegras. La
> **decisión 5 queda reemplazada** por el espejo de RFC-0002: ver más abajo.
>
> El código ya commiteado es neutro respecto a esta resolución, porque `generic`
> preserva la salida 1.x byte a byte con un test que lo fija.

## Contexto

El planificador enruta cada copia a una de tres carpetas operativas fijas,
elegidas únicamente por el tipo de operación:

```rust
fn operational_bucket(operation_type: OperationType) -> Option<&'static str> {
    OperationType::CopyReview    => Some("90_DataForge_Review"),
    OperationType::CopySeparated => Some("95_DataForge_Separated"),
    OperationType::CopyTemporary => Some("98_DataForge_Temporary"),
    _ => None,   // COPY_ACTIVE cuelga de la raíz
}
```

Esa tabla es una decisión de **procedimiento**: dice en qué estado de
tratamiento está un archivo (activo, pendiente de criterio, apartado,
temporal). No dice qué **es**. El resultado es un destino que replica la
estructura del origen partida en cuatro, y nada más.

Sobre un archivo real de despacho —158.219 archivos, 443,9 GB— eso produjo
una salida en la que el 63% del volumen, 280,2 GB, cayó en la bolsa de
revisión. No por un fallo: con perfil `generic` y sin fronteras protegidas,
casi todo cae del lado prudente, que es el comportamiento correcto del
motor tal como está definido hoy.

El resultado que el producto persigue —y que se alcanzó fuera del motor,
con scripts y criterio humano sobre ese mismo corpus— tiene otra forma:
expedientes, correos dentro de su asunto, periciales, recursos formativos y
lo no profesional separado pero visible. Son bolsas con **significado**. El
motor no puede expresarlas, y por tanto no puede producir su propio
resultado esperado.

Dos hechos condicionan cómo se arregla:

- `destination_relative_path` participa en la clave de idempotencia y en el
  manifiesto congelado bajo SHA-256. Cambiar cómo se calcula cambia un
  contrato.
- ADR-0037 congela ese contrato para 1.x y define el único mecanismo de
  cambio: **subir versión y escribir el ADR en el mismo commit, nunca editar
  in place**. La 2.0 es ese vehículo.

## Decisión

1. **Las raíces de destino las declara el perfil, no una constante.** Un
   perfil enumera un conjunto ordenado de raíces, cada una con id estable,
   nombre literal de carpeta y una marca de si forma parte del archivo de
   trabajo o está apartada. `generic` declara exactamente las tres actuales
   más la raíz activa, de modo que su salida no cambia ni un byte.

2. **El tipo de operación sigue acotando *qué*; la raíz decide *dónde*.**
   No se amplía el conjunto de acciones seguras: siguen siendo las cuatro
   copias no destructivas. Una taxonomía puede mandar un archivo a otra
   carpeta; no puede inventar una operación que lo borre. Separar ambos ejes
   es lo que permite enriquecer la salida sin tocar las garantías.

3. **Cada operación registra por qué aterriza donde aterriza.** La raíz
   elegida y la evidencia que la justifica se persisten junto a la
   operación, igual que hoy se persiste `reason`. Una salida bonita cuya
   procedencia no se puede reconstruir no sirve para un archivo probatorio.

4. **Los nombres declarados quedan reservados frente a las raíces de
   origen.** Hoy `root_destination_dirs` reserva las tres constantes para
   que un origen no pueda ensombrecer un contenedor operativo. Esa
   reserva pasa a calcularse sobre el conjunto declarado, que es dinámico.

5. ~~**Lo no clasificable va a la raíz de revisión, nunca a una inventada.**~~
   **Reemplazada por RFC-0002.** Se mantiene el principio —la incertidumbre
   resuelve hacia conservar y hacia señalar, y nunca hacia una raíz inventada—
   pero no la forma. Esta ADR trataba `revisar/` como una bolsa plana, y
   RFC-0002 la define como **espejo del árbol de salida**: lo no clasificable va
   a su mejor ubicación estimada dentro del espejo, con el motivo como metadato,
   y solo cae en `revisar/_sin-ubicar/` cuando no hay ni apuesta de ubicación.

   El motivo del cambio es concreto: en el espejo, aceptar una revisión es mover
   de `revisar/<ruta>` a `output/<ruta>` —la ruta relativa ya está calculada— y
   el revisor ve *junto a qué* iría cada elemento, que es el contexto que
   necesita para decidir. Una bolsa plana pierde eso y convierte cada decisión en
   una búsqueda. Sobre el corpus real la diferencia no es cosmética: la cola son
   5.334 elementos.

6. **Se sube el contrato, no se edita.** Schema de perfil `1.1.0` → `2.0.0`,
   migración nueva para la procedencia de enrutado, y la expectativa del test
   `frozen_contracts` se actualiza en este mismo commit, según ADR-0037 §2.

## Compatibilidad

Un proyecto creado con 1.x y abierto con 2.0 conserva sus planes: las rutas
de destino ya están materializadas como cadenas en `plan_operations`, y un
manifiesto aprobado es inmutable por construcción. Los planes nuevos usan la
taxonomía declarada. No se reescribe historia: un plan de 1.x sigue
verificando contra el mismo manifiesto con el que se aprobó.

## Alternativas consideradas

- **Dejar que el llamante indique la ruta de destino directamente.** Es lo
  más flexible y lo más peligroso: convierte el destino en un parámetro y
  deja al motor sin nada que validar salvo la frontera de escritura. La
  taxonomía declarada mantiene el conjunto de destinos cerrado, enumerable y
  auditable antes de ejecutar.
- **Codificar la taxonomía jurídica dentro del motor.** Descartado: ataría
  el núcleo a un dominio. El perfil ya es el lugar donde este proyecto pone
  lo específico de dominio, y `legal` ya existe.
- **Resolver la clasificación en una capa externa que reescriba la salida
  después.** Es exactamente lo que se hizo con scripts, y funcionó — pero
  los errores los cazó una reauditoría posterior, no el diseño. Dentro del
  motor, la frontera de escritura y el plan congelado impiden la mitad de
  ellos por construcción.
- **Esperar a 2.1 para no romper contratos.** Descartado: el contrato está
  congelado precisamente para que romperlo sea deliberado y documentado, no
  para que sea imposible. Éste es el cambio que justifica el salto mayor.

## Consecuencias

**Positivas.** El motor puede expresar el resultado que persigue. La salida
deja de ser una réplica del origen partida en cuatro. Como las raíces son
declarativas y versionadas, dos ejecuciones del mismo perfil producen la
misma forma, cosa que un clasificador ad-hoc no garantiza.

**Negativas.** Es un cambio de contrato: obliga a migración, a subir el
schema de perfil y a actualizar el inventario congelado. Añade una decisión
que antes no existía —a qué raíz pertenece cada archivo— y con ella una
nueva forma de equivocarse, ahora dentro del ámbito auditado en vez de fuera.

**Neutras.** `generic` mantiene la salida actual byte a byte, de modo que la
2.0 no cambia el comportamiento de quien no adopte un perfil nuevo. La
clasificación semántica propiamente dicha —qué regla reclama qué archivo— no
la fija esta ADR: aquí solo se abre el mecanismo.

**Revisar si.** Si en la práctica los perfiles acaban necesitando declarar
raíces por proyecto y no por dominio, la decisión de anclarlas al perfil
deja de sostenerse y habría que moverlas a la configuración del proyecto.
