# ADR-0003 — El origen es inmutable

**Estado:** Aceptada
**Fecha:** 2026-07-13
**Relacionada con:** RFC-0001 reglas 1–3, §5.2, §27, §37

## Contexto

DataForge trabaja sobre archivos históricos, expedientes y periciales cuyo
valor depende de su integridad. Un fallo que modifique el origen es el peor
resultado posible del producto.

## Decisión

Los orígenes (source roots) son de solo lectura por política en todas las
fases y en todos los componentes:

- `SourceRoot` se construye siempre con `read_only_policy = true`; no existe
  constructor que produzca un origen escribible, y la columna SQLite tiene
  `CHECK (read_only_policy = 1)`;
- el MVP no implementa borrado ni sobrescritura de ningún archivo;
- la fachada valida en la creación del proyecto que ni la carpeta del
  proyecto, ni la salida, ni la auditoría estén dentro de un origen (ni un
  origen dentro de ellas);
- las fases futuras (escaneo, hash, copia) abren los archivos de origen
  exclusivamente en modo lectura y verifican `fingerprint_before ==
  fingerprint_after`;
- toda copia se hará a un destino nuevo con nombre parcial + rename atómico,
  jamás sobre un archivo existente.

## Alternativas consideradas

- **Modo "mover" opcional**: descartado para el MVP; incluso opt-in
  introduce rutas de código destructivas que contradicen las reglas 1–3.
- **Confiar en atributos de solo lectura del sistema de archivos**:
  insuficiente; la garantía debe estar en el código y en las pruebas.

## Consecuencias

- Mayor uso de disco (copiar en lugar de mover) — aceptado por diseño.
- Las pruebas de propiedad de hitos futuros deben incluir "nunca escribe
  dentro de un origen" y "nunca sobrescribe un destino".
- Cualquier excepción futura exige una RFC nueva (cambio mayor, RFC-0001 §50).
