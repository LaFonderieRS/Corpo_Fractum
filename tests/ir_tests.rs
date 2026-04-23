//! Unit tests for rustdec-ir: types, values, functions, blocks, modules.

use rustdec_ir::{BasicBlock, IrFunction, IrModule, IrType, SlotOrigin, Terminator, Value};

// ── IrType::byte_size ─────────────────────────────────────────────────────────

#[test]
fn irtype_byte_size_unsigned_ints() {
    assert_eq!(IrType::UInt(8).byte_size(),   Some(1));
    assert_eq!(IrType::UInt(16).byte_size(),  Some(2));
    assert_eq!(IrType::UInt(32).byte_size(),  Some(4));
    assert_eq!(IrType::UInt(64).byte_size(),  Some(8));
    assert_eq!(IrType::UInt(128).byte_size(), Some(16));
}

#[test]
fn irtype_byte_size_signed_ints() {
    assert_eq!(IrType::SInt(8).byte_size(),  Some(1));
    assert_eq!(IrType::SInt(16).byte_size(), Some(2));
    assert_eq!(IrType::SInt(32).byte_size(), Some(4));
    assert_eq!(IrType::SInt(64).byte_size(), Some(8));
}

#[test]
fn irtype_byte_size_floats() {
    assert_eq!(IrType::Float(32).byte_size(), Some(4));
    assert_eq!(IrType::Float(64).byte_size(), Some(8));
}

#[test]
fn irtype_byte_size_ptr_always_8() {
    assert_eq!(IrType::ptr(IrType::UInt(8)).byte_size(),   Some(8));
    assert_eq!(IrType::ptr(IrType::Unknown).byte_size(),   Some(8));
    assert_eq!(IrType::ptr(IrType::Void).byte_size(),      Some(8));
}

#[test]
fn irtype_byte_size_array() {
    let arr = IrType::Array { elem: Box::new(IrType::UInt(32)), len: 4 };
    assert_eq!(arr.byte_size(), Some(16));

    let nested = IrType::Array { elem: Box::new(IrType::UInt(8)), len: 100 };
    assert_eq!(nested.byte_size(), Some(100));

    // Array of unknown-sized elements → None.
    let unknown_arr = IrType::Array { elem: Box::new(IrType::Unknown), len: 4 };
    assert_eq!(unknown_arr.byte_size(), None);
}

#[test]
fn irtype_byte_size_struct() {
    let s = IrType::Struct { name: "Foo".into(), size: 24 };
    assert_eq!(s.byte_size(), Some(24));

    let zero = IrType::Struct { name: "Empty".into(), size: 0 };
    assert_eq!(zero.byte_size(), Some(0));
}

#[test]
fn irtype_byte_size_void_and_unknown_are_none() {
    assert_eq!(IrType::Void.byte_size(),    None);
    assert_eq!(IrType::Unknown.byte_size(), None);
}

// ── IrType convenience constructors ──────────────────────────────────────────

#[test]
fn irtype_constructors_match_enum_variants() {
    assert_eq!(IrType::u64(), IrType::UInt(64));
    assert_eq!(IrType::u32(), IrType::UInt(32));
    assert_eq!(IrType::u8(),  IrType::UInt(8));
    assert_eq!(IrType::ptr(IrType::UInt(8)), IrType::Ptr(Box::new(IrType::UInt(8))));
}

// ── Value ─────────────────────────────────────────────────────────────────────

#[test]
fn value_display_var() {
    assert_eq!(Value::Var { id: 0,  ty: IrType::UInt(64).into() }.display(), "v0");
    assert_eq!(Value::Var { id: 42, ty: IrType::UInt(32).into() }.display(), "v42");
    assert_eq!(Value::Var { id: 999, ty: IrType::Unknown.into() }.display(), "v999");
}

#[test]
fn value_display_const_hex() {
    assert_eq!(Value::Const { val: 0,      ty: IrType::UInt(64).into() }.display(), "0x0");
    assert_eq!(Value::Const { val: 0x1234, ty: IrType::UInt(64).into() }.display(), "0x1234");
    assert_eq!(Value::Const { val: u64::MAX, ty: IrType::UInt(64).into() }.display(),
               format!("{:#x}", u64::MAX));
}

#[test]
fn value_ty_returns_inner_type() {
    let v = Value::Var   { id: 1, ty: IrType::SInt(32).into() };
    let c = Value::Const { val: 7, ty: IrType::UInt(8).into() };
    assert_eq!(v.ty(), &IrType::SInt(32));
    assert_eq!(c.ty(), &IrType::UInt(8));
}

// ── IrFunction ────────────────────────────────────────────────────────────────

#[test]
fn irfunction_new_has_correct_defaults() {
    let f = IrFunction::new("my_func", 0x400000);
    assert_eq!(f.name,       "my_func");
    assert_eq!(f.entry_addr, 0x400000);
    assert_eq!(f.next_var_id, 0);
    assert!(f.params.is_empty());
    assert_eq!(f.ret_ty, IrType::Unknown);
    assert!(f.slot_table.is_empty());
    assert_eq!(f.frame_size, 0);
    assert!(f.cfg.node_count() == 0);
}

#[test]
fn fresh_var_is_monotonically_increasing() {
    let mut f = IrFunction::new("test", 0);
    assert_eq!(f.fresh_var(), 0);
    assert_eq!(f.fresh_var(), 1);
    assert_eq!(f.fresh_var(), 2);
    assert_eq!(f.next_var_id, 3);
}

#[test]
fn get_or_insert_slot_local_variables() {
    let mut f = IrFunction::new("test", 0);
    {
        let s = f.get_or_insert_slot(-8, IrType::UInt(64));
        assert_eq!(s.name,       "local_0");
        assert_eq!(s.origin,     SlotOrigin::Local);
        assert_eq!(s.rbp_offset, -8);
    }
    {
        let s = f.get_or_insert_slot(-16, IrType::UInt(64));
        assert_eq!(s.name,   "local_1");
        assert_eq!(s.origin, SlotOrigin::Local);
    }
    {
        let s = f.get_or_insert_slot(-24, IrType::UInt(64));
        assert_eq!(s.name,   "local_2");
    }
}

#[test]
fn get_or_insert_slot_saved_rbp_at_zero() {
    let mut f = IrFunction::new("test", 0);
    let s = f.get_or_insert_slot(0, IrType::UInt(64));
    assert_eq!(s.name,   "saved_rbp");
    assert_eq!(s.origin, SlotOrigin::SavedReg);
}

#[test]
fn get_or_insert_slot_stack_args() {
    let mut f = IrFunction::new("test", 0);
    // First stack argument lives at rbp+16 on x86-64 SysV.
    {
        let s = f.get_or_insert_slot(16, IrType::UInt(64));
        assert_eq!(s.name,   "arg_0");
        assert_eq!(s.origin, SlotOrigin::StackArg);
    }
    {
        let s = f.get_or_insert_slot(24, IrType::UInt(64));
        assert_eq!(s.name,   "arg_1");
    }
}

#[test]
fn get_or_insert_slot_type_is_stable_on_repeat_call() {
    let mut f = IrFunction::new("test", 0);
    // UInt(64) is the generic fallback — a more specific type refines it.
    f.get_or_insert_slot(-8, IrType::UInt(64));
    let s = f.get_or_insert_slot(-8, IrType::SInt(32));
    assert_eq!(s.ty, IrType::SInt(32),
        "UInt(64) placeholder must be refined by a more specific type");
    // Once set to a non-generic type, it must not be overwritten.
    let s2 = f.get_or_insert_slot(-8, IrType::UInt(64));
    assert_eq!(s2.ty, IrType::SInt(32),
        "specific type must not regress back to UInt(64)");
}

#[test]
fn get_or_insert_slot_inserts_into_slot_table() {
    let mut f = IrFunction::new("test", 0);
    f.get_or_insert_slot(-8, IrType::UInt(64));
    f.get_or_insert_slot(-16, IrType::UInt(64));
    assert_eq!(f.slot_table.len(), 2);
    assert!(f.slot_table.contains_key(&-8));
    assert!(f.slot_table.contains_key(&-16));
}

#[test]
fn blocks_sorted_on_empty_function() {
    let f = IrFunction::new("empty", 0x400000);
    assert!(f.blocks_sorted().is_empty());
}

// ── BasicBlock ────────────────────────────────────────────────────────────────

#[test]
fn basic_block_new_defaults() {
    let bb = BasicBlock::new(7, 0xdeadbeef);
    assert_eq!(bb.id,         7);
    assert_eq!(bb.start_addr, 0xdeadbeef);
    assert_eq!(bb.end_addr,   0xdeadbeef);
    assert!(bb.stmts.is_empty());
    assert_eq!(bb.confidence, 1.0);
    assert!(matches!(bb.terminator, Terminator::Unreachable));
}

// ── IrModule ──────────────────────────────────────────────────────────────────

#[test]
fn irmodule_default_is_empty() {
    let m = IrModule::default();
    assert!(m.functions.is_empty());
    assert!(m.string_table.is_empty());
}
