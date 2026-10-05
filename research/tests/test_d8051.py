"""Tests for the research disassembler. Fixtures are hand-assembled; no proprietary firmware is used."""

import d8051


def test_every_opcode_is_defined_once() -> None:
    assert len(d8051.OPS) == 256 - 1  # 0xA5 is the only undefined 8051 opcode
    assert 0xA5 not in d8051.OPS


def test_decodes_the_jms578flash_hook_prologue() -> None:
    # MOV DPTR,#7990h ; MOVX A,@DPTR ; CJNE A,#77h,+2Dh  (from jms578flash's hook.asm)
    code = bytes.fromhex("907990e0b4772d")
    texts = [i.text for i in sorted(d8051.explore(code, 0x0000, [0]).values(), key=lambda i: i.address)]
    assert texts[:3] == ["MOV DPTR, #7990h", "MOVX A,@DPTR", "CJNE A, #77h, 0034h"]


def test_relative_and_absolute_targets() -> None:
    assert d8051.decode(bytes.fromhex("80fe"), 0x4000, 0x4000).targets == [0x4000]  # SJMP $
    assert d8051.decode(bytes.fromhex("124ffc"), 0x4000, 0x4000).targets == [0x4FFC]
    assert d8051.decode(bytes.fromhex("3101"), 0x4800, 0x4800).targets == [0x4901]  # ACALL page 1


def test_bit_and_register_forms() -> None:
    assert d8051.decode(bytes.fromhex("30e505"), 0, 0).text == "JNB ACC.5, 0008h"
    assert d8051.decode(bytes.fromhex("e7"), 0, 0).text == "MOV A,@R1"
    assert d8051.decode(bytes.fromhex("f3"), 0, 0).text == "MOVX @R1,A"
    assert d8051.decode(bytes.fromhex("8f37"), 0, 0).text == "MOV 37h, R7"


def test_finds_the_usb2_flag_pattern() -> None:
    # NVRAM byte 0xF3 copied to 0x4420, then bit 5 tested: the shape documented in FINDINGS.md.
    code = bytes.fromhex("903bf3e0904420f0" + "00" * 8 + "904420e030e505")
    loads = d8051.xdata_accesses(code, 0x4000, 0x3BF3)
    tests = d8051.xdata_accesses(code, 0x4000, 0x4420)
    assert loads == [(0x4000, "read")]
    assert tests == [(0x4004, "write"), (0x4010, "read")]
