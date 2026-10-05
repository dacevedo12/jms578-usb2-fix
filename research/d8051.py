#!/usr/bin/env python3
"""Minimal 8051 disassembler for JMS578 firmware research.

Reads a flash backup made by jms578-usb2-fix (or any dump starting at flash address 0), extracts the 8051
program (flash 0x1000, loaded at code address 0x4000), and can list code reachable from the firmware's entry
stubs or show every access to an XDATA address.

    python3 d8051.py backup.bin --xref 3BF3      # who reads NVRAM byte 0xF3
    python3 d8051.py backup.bin --xref 4420      # who tests the USB 2.0-only flag
    python3 d8051.py backup.bin --list --linear --from 4D43 --to 4E70   # link state machine
"""

from __future__ import annotations

import argparse
import sys
from dataclasses import dataclass, field

CODE_FLASH_START = 0x1000
CODE_LOAD_ADDRESS = 0x4000
CODE_LENGTH = 0xC000 - 8
ENTRY_STUBS = tuple(range(0x4000, 0x4080, 0x10))

SFR = {
    0x81: "SP", 0x82: "DPL", 0x83: "DPH", 0x87: "PCON", 0x88: "TCON", 0x89: "TMOD", 0x90: "P1",
    0x98: "SCON", 0x99: "SBUF", 0xA0: "P2", 0xA8: "IE", 0xB0: "P3", 0xB8: "IP", 0xD0: "PSW",
    0xE0: "ACC", 0xF0: "B",
}  # fmt: skip

# opcode -> (mnemonic, length, operand kinds)
# kinds: d direct, i #imm8, I #imm16, r rel8, a11 ajmp/acall, a16 ljmp/lcall, b bit, D2 "mov dir,dir"
OPS: dict[int, tuple[str, int, tuple[str, ...]]] = {}


def _op(code: int, mnemonic: str, length: int, *kinds: str) -> None:
    OPS[code] = (mnemonic, length, kinds)


for _code, _m, _n, _k in [
    (0x00, "NOP", 1, ()), (0x02, "LJMP", 3, ("a16",)), (0x03, "RR A", 1, ()), (0x04, "INC A", 1, ()),
    (0x05, "INC", 2, ("d",)), (0x10, "JBC", 3, ("b", "r")), (0x12, "LCALL", 3, ("a16",)),
    (0x13, "RRC A", 1, ()), (0x14, "DEC A", 1, ()), (0x15, "DEC", 2, ("d",)), (0x20, "JB", 3, ("b", "r")),
    (0x22, "RET", 1, ()), (0x23, "RL A", 1, ()), (0x24, "ADD A,", 2, ("i",)), (0x25, "ADD A,", 2, ("d",)),
    (0x30, "JNB", 3, ("b", "r")), (0x32, "RETI", 1, ()), (0x33, "RLC A", 1, ()), (0x34, "ADDC A,", 2, ("i",)),
    (0x35, "ADDC A,", 2, ("d",)), (0x40, "JC", 2, ("r",)), (0x42, "ORL", 2, ("d", "A")),
    (0x43, "ORL", 3, ("d", "i")), (0x44, "ORL A,", 2, ("i",)), (0x45, "ORL A,", 2, ("d",)),
    (0x50, "JNC", 2, ("r",)), (0x52, "ANL", 2, ("d", "A")), (0x53, "ANL", 3, ("d", "i")),
    (0x54, "ANL A,", 2, ("i",)), (0x55, "ANL A,", 2, ("d",)), (0x60, "JZ", 2, ("r",)),
    (0x62, "XRL", 2, ("d", "A")), (0x63, "XRL", 3, ("d", "i")), (0x64, "XRL A,", 2, ("i",)),
    (0x65, "XRL A,", 2, ("d",)), (0x70, "JNZ", 2, ("r",)), (0x72, "ORL C,", 2, ("b",)),
    (0x73, "JMP @A+DPTR", 1, ()), (0x74, "MOV A,", 2, ("i",)), (0x75, "MOV", 3, ("d", "i")),
    (0x80, "SJMP", 2, ("r",)), (0x82, "ANL C,", 2, ("b",)), (0x83, "MOVC A,@A+PC", 1, ()),
    (0x84, "DIV AB", 1, ()), (0x85, "MOV", 3, ("D2",)), (0x90, "MOV DPTR,", 3, ("I",)),
    (0x92, "MOV", 2, ("b", "C")), (0x93, "MOVC A,@A+DPTR", 1, ()), (0x94, "SUBB A,", 2, ("i",)),
    (0x95, "SUBB A,", 2, ("d",)), (0xA0, "ORL C,/", 2, ("b",)), (0xA2, "MOV C,", 2, ("b",)),
    (0xA3, "INC DPTR", 1, ()), (0xA4, "MUL AB", 1, ()), (0xB0, "ANL C,/", 2, ("b",)), (0xB2, "CPL", 2, ("b",)),
    (0xB3, "CPL C", 1, ()), (0xB4, "CJNE A,", 3, ("i", "r")), (0xB5, "CJNE A,", 3, ("d", "r")),
    (0xC0, "PUSH", 2, ("d",)), (0xC2, "CLR", 2, ("b",)), (0xC3, "CLR C", 1, ()), (0xC4, "SWAP A", 1, ()),
    (0xC5, "XCH A,", 2, ("d",)), (0xD0, "POP", 2, ("d",)), (0xD2, "SETB", 2, ("b",)), (0xD3, "SETB C", 1, ()),
    (0xD4, "DA A", 1, ()), (0xD5, "DJNZ", 3, ("d", "r")), (0xE0, "MOVX A,@DPTR", 1, ()),
    (0xE4, "CLR A", 1, ()), (0xE5, "MOV A,", 2, ("d",)), (0xF0, "MOVX @DPTR,A", 1, ()), (0xF4, "CPL A", 1, ()),
    (0xF5, "MOV", 2, ("d", "A")),
]:  # fmt: skip
    _op(_code, _m, _n, *_k)

for _i in (0, 1):  # @R0 / @R1 forms: low opcode bit selects the register
    for _base, _m, _n, _k in [
        (0x06, "INC @R{}", 1, ()), (0x16, "DEC @R{}", 1, ()), (0x26, "ADD A,@R{}", 1, ()),
        (0x36, "ADDC A,@R{}", 1, ()), (0x46, "ORL A,@R{}", 1, ()), (0x56, "ANL A,@R{}", 1, ()),
        (0x66, "XRL A,@R{}", 1, ()), (0x76, "MOV @R{},", 2, ("i",)), (0x86, "MOV", 2, ("d", "@R{}")),
        (0x96, "SUBB A,@R{}", 1, ()), (0xA6, "MOV @R{},", 2, ("d",)), (0xB6, "CJNE @R{},", 3, ("i", "r")),
        (0xC6, "XCH A,@R{}", 1, ()), (0xD6, "XCHD A,@R{}", 1, ()), (0xE2, "MOVX A,@R{}", 1, ()),
        (0xE6, "MOV A,@R{}", 1, ()), (0xF2, "MOVX @R{},A", 1, ()), (0xF6, "MOV @R{},A", 1, ()),
    ]:  # fmt: skip
        _op(_base + _i, _m.format(_i), _n, *(k.format(_i) for k in _k))

for _r in range(8):
    for _base, _m, _n, _k in [
        (0x08, "INC R{}", 1, ()), (0x18, "DEC R{}", 1, ()), (0x28, "ADD A,R{}", 1, ()),
        (0x38, "ADDC A,R{}", 1, ()), (0x48, "ORL A,R{}", 1, ()), (0x58, "ANL A,R{}", 1, ()),
        (0x68, "XRL A,R{}", 1, ()), (0x78, "MOV R{},", 2, ("i",)), (0x88, "MOV", 2, ("d", "R{}")),
        (0x98, "SUBB A,R{}", 1, ()), (0xA8, "MOV R{},", 2, ("d",)), (0xB8, "CJNE R{},", 3, ("i", "r")),
        (0xC8, "XCH A,R{}", 1, ()), (0xD8, "DJNZ R{},", 2, ("r",)), (0xE8, "MOV A,R{}", 1, ()),
        (0xF8, "MOV R{},A", 1, ()),
    ]:  # fmt: skip
        _op(_base + _r, _m.format(_r), _n, *(k.format(_r) for k in _k))

for _page in range(8):
    _op(_page * 0x20 + 0x01, "AJMP", 2, "a11")
    _op(_page * 0x20 + 0x11, "ACALL", 2, "a11")


@dataclass
class Insn:
    address: int
    raw: bytes
    mnemonic: str
    text: str
    targets: list[int] = field(default_factory=list)
    imm16: int | None = None
    imm8: int | None = None

    @property
    def ends_flow(self) -> bool:
        return self.mnemonic in ("LJMP", "AJMP", "SJMP", "RET", "RETI", "JMP @A+DPTR")


def _direct(value: int) -> str:
    return SFR.get(value, f"{value:02X}h")


def _bit(value: int) -> str:
    if value < 0x80:
        return f"{0x20 + value // 8:02X}h.{value % 8}"
    return f"{SFR.get(value & 0xF8, f'{value & 0xF8:02X}h')}.{value & 7}"


def decode(code: bytes, base: int, address: int) -> Insn | None:
    """Decodes the instruction at `address` (code[0] is at `base`)."""
    offset = address - base
    if not 0 <= offset < len(code):
        return None
    opcode = code[offset]
    mnemonic, length, kinds = OPS.get(opcode, (f"DB {opcode:02X}h", 1, ()))
    raw = code[offset : offset + length]
    if len(raw) < length:
        return None
    insn = Insn(address, raw, mnemonic, mnemonic)
    operands: list[str] = []
    index = 1
    for kind in kinds:
        if kind == "d":
            operands.append(_direct(raw[index]))
            index += 1
        elif kind == "i":
            insn.imm8 = raw[index]
            operands.append(f"#{raw[index]:02X}h")
            index += 1
        elif kind == "I":
            insn.imm16 = raw[1] << 8 | raw[2]
            operands.append(f"#{insn.imm16:04X}h")
            index += 2
        elif kind == "b":
            operands.append(_bit(raw[index]))
            index += 1
        elif kind == "r":
            delta = raw[index] - 256 if raw[index] > 127 else raw[index]
            target = (address + length + delta) & 0xFFFF
            insn.targets.append(target)
            operands.append(f"{target:04X}h")
            index += 1
        elif kind == "a16":
            target = raw[1] << 8 | raw[2]
            insn.targets.append(target)
            operands.append(f"{target:04X}h")
            index += 2
        elif kind == "a11":
            target = ((address + 2) & 0xF800) | ((opcode & 0xE0) << 3) | raw[1]
            insn.targets.append(target)
            operands.append(f"{target:04X}h")
            index += 1
        elif kind == "D2":  # MOV dir,dir encodes the source first
            operands += [_direct(raw[2]), _direct(raw[1])]
        else:
            operands.append(kind)
    if operands:
        insn.text = f"{mnemonic} {', '.join(operands)}"
    return insn


def explore(code: bytes, base: int, entries: list[int]) -> dict[int, Insn]:
    """Recursive-descent disassembly from `entries`."""
    seen: dict[int, Insn] = {}
    work = list(entries)
    while work:
        address = work.pop()
        while address not in seen:
            insn = decode(code, base, address)
            if insn is None:
                break
            seen[address] = insn
            work.extend(insn.targets)
            if insn.ends_flow:
                break
            address += len(insn.raw)
    return seen


def linear(code: bytes, base: int, start: int, end: int) -> list[Insn]:
    """Decodes straight through a range (for code only reachable via jump tables)."""
    out = []
    address = start
    while address <= end and (insn := decode(code, base, address)) is not None:
        out.append(insn)
        address += len(insn.raw)
    return out


def call_targets(code: bytes, base: int) -> list[int]:
    """LCALL targets appearing at least twice: a cheap way to find functions behind jump tables."""
    counts: dict[int, int] = {}
    for offset in range(len(code) - 2):
        if code[offset] == 0x12:
            target = code[offset + 1] << 8 | code[offset + 2]
            if base <= target < base + len(code):
                counts[target] = counts.get(target, 0) + 1
    return sorted(t for t, n in counts.items() if n >= 2 and code[t - base] not in (0x00, 0xFF))


def xdata_accesses(code: bytes, base: int, target: int) -> list[tuple[int, str]]:
    """Every `MOV DPTR,#target` followed by a MOVX read or write, found by scanning all offsets."""
    found = []
    pattern = bytes([0x90, target >> 8, target & 0xFF])
    start = 0
    while (offset := code.find(pattern, start)) != -1:
        start = offset + 1
        following = decode(code, base, base + offset + 3)
        if following and following.mnemonic.startswith("MOVX"):
            found.append((base + offset, "write" if following.mnemonic == "MOVX @DPTR,A" else "read"))
    return found


def code_from_flash(flash: bytes) -> bytes:
    return flash[CODE_FLASH_START : CODE_FLASH_START + CODE_LENGTH]


def main(argv: list[str] | None = None) -> int:
    parser = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    parser.add_argument("dump", help="flash backup .bin (starting at flash address 0)")
    parser.add_argument("--xref", type=lambda s: int(s, 16), help="hex XDATA address to cross-reference")
    parser.add_argument("--list", action="store_true", help="list code reachable from the entry stubs")
    parser.add_argument("--linear", action="store_true", help="with --list: decode straight from --from to --to")
    parser.add_argument("--from", dest="start", type=lambda s: int(s, 16), default=0)
    parser.add_argument("--to", dest="end", type=lambda s: int(s, 16), default=0xFFFF)
    args = parser.parse_args(argv)

    with open(args.dump, "rb") as handle:
        code = code_from_flash(handle.read())
    if args.xref is not None:
        for address, kind in xdata_accesses(code, CODE_LOAD_ADDRESS, args.xref):
            after = decode(code, CODE_LOAD_ADDRESS, address + 4)
            print(f"{address:04X}  {kind:5}  then: {after.text if after else '?'}")
    if args.list and args.linear:
        for insn in linear(code, CODE_LOAD_ADDRESS, args.start, args.end):
            print(f"{insn.address:04X}  {insn.raw.hex():<8}  {insn.text}")
    elif args.list:
        entries = list(ENTRY_STUBS) + call_targets(code, CODE_LOAD_ADDRESS)
        for address, insn in sorted(explore(code, CODE_LOAD_ADDRESS, entries).items()):
            if args.start <= address <= args.end:
                print(f"{address:04X}  {insn.raw.hex():<8}  {insn.text}")
    return 0


if __name__ == "__main__":
    sys.exit(main())
