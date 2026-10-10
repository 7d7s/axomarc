;; Wave 9 B-30 fixture: capcheck — exercises host.capability_check.
;;
;; Input: a JSON string, either "action" or "action|path" (e.g. "probe",
;; "probe|LE1", "admin.wipe"). The guest strips the quotes, splits on the
;; first '|' and asks the host whether the action is allowed for the
;; current execution scope. Output: {"allowed":true} / {"allowed":false}.
(module
  (import "host" "log" (func $log (param i32 i32)))
  (import "host" "capability_check"
    (func $capability_check (param i32 i32 i32 i32) (result i32)))

  (memory (export "memory") 2)
  (global $heap (mut i32) (i32.const 16384))

  (data (i32.const 1024)
    "{\"module\":{\"name\":\"capcheck\"},\"capabilities\":{\"probe\":{\"profile\":\"READ\"}}}\00")
  (data (i32.const 1200) "{\"allowed\":true}")     ;; 16 bytes (1200..1216)
  (data (i32.const 1224) "{\"allowed\":false}")    ;; 17 bytes (1224..1241)
  (data (i32.const 1256) "capcheck invoked")       ;; 16 bytes (1256..1272)

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
    (local $i i32)
    (local $split i32)     ;; offset of the first '|' or -1
    (local $act i32)
    (local $act_len i32)
    (local $path i32)
    (local $path_len i32)
    (local $allowed i32)
    (local $out i32)
    (call $log (i32.const 1256) (i32.const 16))
    ;; strip the JSON string quotes: content = [in+1, in+in_len-1)
    (if (i32.lt_u (local.get $in_len) (i32.const 2))
      (then
        (local.set $span (local.get $in))
        (local.set $end (local.get $in)))
      (else
        (local.set $span (i32.add (local.get $in) (i32.const 1)))
        (local.set $end (i32.add (local.get $in) (i32.sub (local.get $in_len) (i32.const 1))))))
    ;; find the first '|'
    (local.set $split (i32.const -1))
    (local.set $i (local.get $span))
    (block $found
      (loop $l
        (br_if $found (i32.ge_u (local.get $i) (local.get $end)))
        (if (i32.eq (i32.load8_u (local.get $i)) (i32.const 124))
          (then
            (local.set $split (local.get $i))
            (br $found)))
        (local.set $i (i32.add (local.get $i) (i32.const 1)))
        (br $l)))
    (if (i32.ge_s (local.get $split) (i32.const 0))
      (then
        (local.set $act (local.get $span))
        (local.set $act_len (i32.sub (local.get $split) (local.get $span)))
        (local.set $path (i32.add (local.get $split) (i32.const 1)))
        (local.set $path_len (i32.sub (local.get $end) (local.get $path))))
      (else
        (local.set $act (local.get $span))
        (local.set $act_len (i32.sub (local.get $end) (local.get $span)))
        (local.set $path (i32.const 0))
        (local.set $path_len (i32.const 0))))
    (local.set $allowed
      (call $capability_check
        (local.get $act) (local.get $act_len)
        (local.get $path) (local.get $path_len)))
    (if (local.get $allowed)
      (then
        (local.set $out (call $alloc (i32.const 16)))
        (if (i32.eqz (local.get $out)) (then (unreachable)))
        (call $copy (local.get $out) (i32.const 1200) (i32.const 16))
        (return (i64.or
                  (i64.shl (i64.extend_i32_u (local.get $out)) (i64.const 32))
                  (i64.extend_i32_u (i32.const 16)))))
      (else
        (local.set $out (call $alloc (i32.const 17)))
        (if (i32.eqz (local.get $out)) (then (unreachable)))
        (call $copy (local.get $out) (i32.const 1224) (i32.const 17))
        (return (i64.or
                  (i64.shl (i64.extend_i32_u (local.get $out)) (i64.const 32))
                  (i64.extend_i32_u (i32.const 17))))))
    (unreachable)))
