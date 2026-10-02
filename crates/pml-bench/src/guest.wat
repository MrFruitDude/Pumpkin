;; Benchmark guest for the PML P0 hook-latency measurements.
;;
;; It is a WebAssembly *component* with the same shape a PML mod has on
;; Pumpkin's plugin host: sync-lifted exports the host calls as event/block
;; hooks, and imports the guest calls back into the host. Bodies are kept
;; trivial on purpose so that what is measured is the boundary, not the work.
;;
;; Exports
;;   hook(id, x, y, z, state) -> u32   a block-hook-shaped call (scalars only)
;;   call-host-async(n) -> u32         calls `host-noop-async` n times
;;   call-host-sync(n) -> u32          calls `host-noop-sync` n times
;;   list-len(data: list<u8>) -> u32   record/payload copy into guest memory
;;   tick-batch(handles: list<u64>) -> u64
;;                                     one batched block-entity tick: touches
;;                                     every handle once
;;   spin(n) -> u32                    pure guest compute loop (epoch cost)
(component
  (import "host-noop-async" (func $host_noop_async (param "x" u32) (result u32)))
  (import "host-noop-sync" (func $host_noop_sync (param "x" u32) (result u32)))

  (core module $guest
    (import "host" "noop-async" (func $noop_async (param i32) (result i32)))
    (import "host" "noop-sync" (func $noop_sync (param i32) (result i32)))

    (memory (export "memory") 2)
    (global $ticked (mut i64) (i64.const 0))

    ;; Every list argument is copied to one fixed scratch region starting at
    ;; 64 KiB; a call only ever takes one list, so nothing has to be freed.
    (func (export "cabi_realloc")
      (param $old i32) (param $old_size i32) (param $align i32) (param $new_size i32)
      (result i32)
      (local $missing i32)
      ;; pages needed for [65536, 65536 + new_size) minus pages present
      (local.set $missing
        (i32.sub
          (i32.shr_u
            (i32.add (i32.add (i32.const 65536) (local.get $new_size)) (i32.const 65535))
            (i32.const 16))
          (memory.size)))
      (if (i32.gt_s (local.get $missing) (i32.const 0))
        (then
          (if (i32.eq (memory.grow (local.get $missing)) (i32.const -1))
            (then (unreachable)))))
      (i32.const 65536))

    (func (export "hook")
      (param $id i32) (param $x i32) (param $y i32) (param $z i32) (param $state i32)
      (result i32)
      (i32.xor
        (i32.add (local.get $id) (local.get $state))
        (i32.add (local.get $x) (i32.add (local.get $y) (local.get $z)))))

    (func (export "call-host-async") (param $n i32) (result i32)
      (local $i i32) (local $acc i32)
      (block $done
        (loop $next
          (br_if $done (i32.ge_u (local.get $i) (local.get $n)))
          (local.set $acc (call $noop_async (local.get $acc)))
          (local.set $i (i32.add (local.get $i) (i32.const 1)))
          (br $next)))
      (local.get $acc))

    (func (export "call-host-sync") (param $n i32) (result i32)
      (local $i i32) (local $acc i32)
      (block $done
        (loop $next
          (br_if $done (i32.ge_u (local.get $i) (local.get $n)))
          (local.set $acc (call $noop_sync (local.get $acc)))
          (local.set $i (i32.add (local.get $i) (i32.const 1)))
          (br $next)))
      (local.get $acc))

    (func (export "list-len") (param $ptr i32) (param $len i32) (result i32)
      (local.get $len))

    (func (export "tick-batch") (param $ptr i32) (param $len i32) (result i64)
      (local $i i32) (local $acc i64)
      (local.set $acc (global.get $ticked))
      (block $done
        (loop $next
          (br_if $done (i32.ge_u (local.get $i) (local.get $len)))
          (local.set $acc
            (i64.add
              (local.get $acc)
              (i64.load (i32.add (local.get $ptr) (i32.shl (local.get $i) (i32.const 3))))))
          (local.set $i (i32.add (local.get $i) (i32.const 1)))
          (br $next)))
      (global.set $ticked (local.get $acc))
      (local.get $acc))

    (func (export "spin") (param $n i32) (result i32)
      (local $i i32) (local $x i32)
      (local.set $x (i32.const 2463534242))
      (block $done
        (loop $next
          (br_if $done (i32.ge_u (local.get $i) (local.get $n)))
          ;; xorshift32
          (local.set $x (i32.xor (local.get $x) (i32.shl (local.get $x) (i32.const 13))))
          (local.set $x (i32.xor (local.get $x) (i32.shr_u (local.get $x) (i32.const 17))))
          (local.set $x (i32.xor (local.get $x) (i32.shl (local.get $x) (i32.const 5))))
          (local.set $i (i32.add (local.get $i) (i32.const 1)))
          (br $next)))
      (local.get $x))
  )

  (core func $noop_async_lowered (canon lower (func $host_noop_async)))
  (core func $noop_sync_lowered (canon lower (func $host_noop_sync)))
  (core instance $host
    (export "noop-async" (func $noop_async_lowered))
    (export "noop-sync" (func $noop_sync_lowered)))
  (core instance $g (instantiate $guest (with "host" (instance $host))))

  (func (export "hook")
    (param "id" u32) (param "x" s32) (param "y" s32) (param "z" s32) (param "state" u32)
    (result u32)
    (canon lift (core func $g "hook")))
  (func (export "call-host-async") (param "n" u32) (result u32)
    (canon lift (core func $g "call-host-async")))
  (func (export "call-host-sync") (param "n" u32) (result u32)
    (canon lift (core func $g "call-host-sync")))
  (func (export "list-len") (param "data" (list u8)) (result u32)
    (canon lift (core func $g "list-len")
      (memory (core memory $g "memory")) (realloc (core func $g "cabi_realloc"))))
  (func (export "tick-batch") (param "handles" (list u64)) (result u64)
    (canon lift (core func $g "tick-batch")
      (memory (core memory $g "memory")) (realloc (core func $g "cabi_realloc"))))
  (func (export "spin") (param "n" u32) (result u32)
    (canon lift (core func $g "spin")))
)
