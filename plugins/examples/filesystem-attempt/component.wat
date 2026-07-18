(component
  ;; Deliberately unavailable. DataForge ABI 0.1 links no WASI interfaces.
  (type $attempt (func))
  (import "wasi:filesystem/preopens@0.2.0" (func $attempt (type $attempt)))
  (core module $guest
    (memory (export "memory") 1)
    (func (export "realloc") (param i32 i32 i32 i32) (result i32) i32.const 4096)
    (func (export "describe") (result i32) i32.const 0)
    (func (export "analyze") (param i32 i32) (result i32) i32.const 0)
  )
  (core instance $guest (instantiate $guest))
  (func (export "describe") (result string)
    (canon lift (core func $guest "describe")
      (memory $guest "memory")
      (realloc (func $guest "realloc"))))
  (func (export "analyze") (param "input" string) (result string)
    (canon lift (core func $guest "analyze")
      (memory $guest "memory")
      (realloc (func $guest "realloc"))))
)

