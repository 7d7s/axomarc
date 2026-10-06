;; Wave 9 B-30 fixture: spin — an unconditional infinite loop. Dispatching
;; it must trap with Trap::OutOfFuel and surface as KER_WASM_FUEL (pinned
;; HTTP 503), proving the metering bound. Never run it on the default
;; budget in a test; the host's fuel cap is the only brake.
(module
  (memory (export "memory") 2)
  (global $heap (mut i32) (i32.const 16384))

  (data (i32.const 1024)
    "{\"module\":{\"name\":\"spin\"},\"capabilities\":{\"spin\":{\"profile\":\"READ\"}}}\00")

  (func $alloc (export "keel_alloc") (param $n i32) (result i32)
    (local $p i32)
    (local.set $p (global.get $heap))
    (global.set $heap
      (i32.add (local.get $p)
               (i32.and (i32.add (local.get $n) (i32.const 7)) (i32.const -8))))
    (if (i32.gt_u (global.get $heap) (i32.const 131072))
      (then
        (global.set $heap (local.get $p))
        (return (i32.const 0))))
    (local.get $p))

  (func (export "keel_manifest") (result i64)
    (local $p i32)
    (local.set $p (i32.const 1024))
    (block $done
      (loop $scan
        (br_if $done (i32.eqz (i32.load8_u (local.get $p))))
        (local.set $p (i32.add (local.get $p) (i32.const 1)))
        (br $scan)))
    (i64.or
      (i64.shl (i64.extend_i32_u (i32.const 1024)) (i64.const 32))
      (i64.extend_i32_u (i32.sub (local.get $p) (i32.const 1024)))))

  (func (export "keel_call")
        (param $op i32) (param $op_len i32) (param $in i32) (param $in_len i32)
        (result i64)
    (loop $spin (br $spin))
    (unreachable)))
