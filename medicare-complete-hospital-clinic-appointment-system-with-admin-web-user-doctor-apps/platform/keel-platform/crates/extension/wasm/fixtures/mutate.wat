;; Phase 3 §5.3 fixture: mutate — a COMMAND-profiled guest exercising
;; host.capability_write + host.capability_audit.
;;
;; Every kelson_call attempts one write and one audit of static payloads
;; under the action named by the input JSON string, and returns the host
;; verdicts: {"write":W,"audit":A} with W/A = 1 recorded, 0 refused. The
;; host-side tests assert the verdicts AND the committed effects (delta
;; stone + audit trail via the execution unit).
(module
  (import "host" "log" (func $log (param i32 i32)))
  (import "host" "capability_write"
    (func $capability_write (param i32 i32 i32 i32) (result i32)))
  (import "host" "capability_audit"
    (func $capability_audit (param i32 i32 i32 i32) (result i32)))

  (memory (export "memory") 2)
  (global $heap (mut i32) (i32.const 16384))

  (data (i32.const 1024)
    "{\"module\":{\"name\":\"mutate\",\"version\":\"0.1.0\",\"api\":1},\"capabilities\":{\"write_all\":{\"profile\":\"COMMAND\"}}}\00")
  (data (i32.const 1200) "write_all")                                    ;; 9 bytes
  (data (i32.const 1220) "{\"resource_type\":\"notes\",\"after\":{\"text\":\"hi\"}}") ;; 47 bytes
  (data (i32.const 1280) "{\"action\":\"notes.create\"}")                 ;; 25 bytes
  (data (i32.const 1320) "{\"write\":")                                   ;; 9 bytes
  (data (i32.const 1340) ",\"audit\":")                                   ;; 9 bytes
  (data (i32.const 1360) "}")                                            ;; 1 byte
  (data (i32.const 1380) "mutate invoked")                               ;; 14 bytes

  (func $alloc (export "kelson_alloc") (param $n i32) (result i32)
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

  (func $copy (param $dst i32) (param $src i32) (param $n i32)
    (local $i i32)
    (block $done
      (loop $l
        (br_if $done (i32.ge_u (local.get $i) (local.get $n)))
        (i32.store8 (i32.add (local.get $dst) (local.get $i))
                    (i32.load8_u (i32.add (local.get $src) (local.get $i))))
        (local.set $i (i32.add (local.get $i) (i32.const 1)))
        (br $l))))

  (func (export "kelson_manifest") (result i64)
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

  (func (export "kelson_call")
        (param $op i32) (param $op_len i32) (param $in i32) (param $in_len i32)
        (result i64)
    (local $span i32)      ;; first content byte (quotes stripped)
    (local $end i32)       ;; one past the last content byte
    (local $w i32)
    (local $a i32)
    (local $out i32)
    (call $log (i32.const 1272) (i32.const 14))
    ;; The input JSON string names the action ("write_all" declared,
    ;; anything else undeclared): strip the quotes and use the content
    ;; as the action for both the write and the audit. The host verdicts
    ;; (1 recorded / 0 refused) are echoed back so tests observe the gate
    ;; without host introspection.
    (if (i32.lt_u (local.get $in_len) (i32.const 2))
      (then
        (local.set $span (local.get $in))
        (local.set $end (local.get $in)))
      (else
        (local.set $span (i32.add (local.get $in) (i32.const 1)))
        (local.set $end (i32.add (local.get $in) (i32.sub (local.get $in_len) (i32.const 1))))))
    (local.set $w
      (call $capability_write
        (local.get $span) (i32.sub (local.get $end) (local.get $span))
        (i32.const 1140) (i32.const 47)))
    (local.set $a
      (call $capability_audit
        (local.get $span) (i32.sub (local.get $end) (local.get $span))
        (i32.const 1200) (i32.const 25)))
    ;; {"write":W,"audit":A} — 9 + 1 + 9 + 1 + 1 = 21 bytes.
    (local.set $out (call $alloc (i32.const 21)))
    (if (i32.eqz (local.get $out)) (then (unreachable)))
    (call $copy (local.get $out) (i32.const 1240) (i32.const 9))
    (i32.store8 (i32.add (local.get $out) (i32.const 9))
                (i32.add (local.get $w) (i32.const 48)))
    (call $copy (i32.add (local.get $out) (i32.const 10)) (i32.const 1252) (i32.const 9))
    (i32.store8 (i32.add (local.get $out) (i32.const 19))
                (i32.add (local.get $a) (i32.const 48)))
    (call $copy (i32.add (local.get $out) (i32.const 20)) (i32.const 1264) (i32.const 1))
    (i64.or
      (i64.shl (i64.extend_i32_u (local.get $out)) (i64.const 32))
      (i64.extend_i32_u (i32.const 21)))))
