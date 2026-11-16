//! Compiled Papyrus for the preview: header, string table, properties, variables, functions. @sat:formats.preview.pex
//!
//! Owns: reading PEX (Skyrim's opcodes walked to find function ends), compile facts, the parent script as a link.

use super::{Cur, Kind, Preview};

struct P<'a> {
    c: Cur<'a>,
    strings: Vec<String>,
    le: bool,
}

impl P<'_> {
    fn u8(&mut self) -> Option<u8> {
        self.c.u8()
    }
    fn u16(&mut self) -> Option<u16> {
        if self.le { self.c.u16() } else { self.c.u16be() }
    }
    fn u32(&mut self) -> Option<u32> {
        if self.le { self.c.u32() } else { self.c.u32be() }
    }
    fn u64(&mut self) -> Option<u64> {
        if self.le { self.c.u64() } else { self.c.u64be() }
    }
    fn wstr(&mut self) -> Option<String> {
        let n = self.u16()? as usize;
        self.c.text(n)
    }
    fn s(&mut self) -> Option<String> {
        let i = self.u16()? as usize;
        Some(self.strings.get(i).cloned().unwrap_or_else(|| format!("#{i}")))
    }
    /// A variable data value, as text.
    fn value(&mut self) -> Option<String> {
        Some(match self.u8()? {
            0 => "None".into(),
            1 => self.s()?,
            2 => format!("\"{}\"", self.s()?),
            3 => (self.u32()? as i32).to_string(),
            4 => f32::from_bits(self.u32()?).to_string(),
            5 => (self.u8()? != 0).to_string(),
            t => return self.fail(t),
        })
    }
    fn fail<T>(&mut self, _t: u8) -> Option<T> {
        None
    }
    /// A function: signature text; instructions are walked to find the end.
    fn function(&mut self, name: &str) -> Option<String> {
        let ret = self.s()?;
        let _doc = self.s()?;
        let _uflags = self.u32()?;
        let flags = self.u8()?;
        let np = self.u16()?;
        let mut params = Vec::new();
        for _ in 0..np {
            let n = self.s()?;
            let t = self.s()?;
            params.push(format!("{t} {n}"));
        }
        let nl = self.u16()?;
        for _ in 0..nl {
            self.s()?;
            self.s()?;
        }
        let ni = self.u16()?;
        for _ in 0..ni {
            let op = self.u8()?;
            let (fixed, var) = OPS.get(op as usize).copied()?;
            for _ in 0..fixed {
                self.value()?;
            }
            if var {
                // the count is an integer value
                let t = self.u8()?;
                if t != 3 {
                    return None;
                }
                let n = self.u32()?;
                for _ in 0..n.min(256) {
                    self.value()?;
                }
            }
        }
        let mut sig = format!("{name}({})", params.join(", "));
        if !ret.eq_ignore_ascii_case("none") && !ret.is_empty() {
            sig = format!("{ret} {sig}");
        }
        if flags & 1 != 0 {
            sig = format!("global {sig}");
        }
        if flags & 2 != 0 {
            sig = format!("native {sig}");
        }
        Some(sig)
    }
}

/// Skyrim opcodes: (fixed arguments, has a variable tail).
const OPS: [(u8, bool); 36] = [
    (0, false), // nop
    (3, false), // iadd
    (3, false), // fadd
    (3, false), // isub
    (3, false), // fsub
    (3, false), // imul
    (3, false), // fmul
    (3, false), // idiv
    (3, false), // fdiv
    (3, false), // imod
    (2, false), // not
    (2, false), // ineg
    (2, false), // fneg
    (2, false), // assign
    (2, false), // cast
    (3, false), // cmp_eq
    (3, false), // cmp_lt
    (3, false), // cmp_le
    (3, false), // cmp_gt
    (3, false), // cmp_ge
    (1, false), // jmp
    (2, false), // jmpt
    (2, false), // jmpf
    (3, true),  // callmethod
    (2, true),  // callparent
    (3, true),  // callstatic
    (1, false), // return
    (3, false), // strcat
    (3, false), // propget
    (3, false), // propset
    (2, false), // array_create
    (2, false), // array_length
    (3, false), // array_getelement
    (3, false), // array_setelement
    (4, false), // array_findelement
    (4, false), // array_rfindelement
];

pub(super) fn read(b: &[u8]) -> Preview {
    let mut p = Preview::new(Kind::Script, "PEX");
    let le = match b.get(..4) {
        Some([0xFA, 0x57, 0xC0, 0xDE]) => false,
        Some([0xDE, 0xC0, 0x57, 0xFA]) => true,
        _ => {
            p.note("это не PEX");
            return p;
        }
    };
    let mut r = P {
        c: Cur::new(b),
        strings: Vec::new(),
        le,
    };
    r.c.at = 4;
    if header(&mut r, &mut p).is_none() {
        p.note("файл прочитан не полностью");
    }
    p
}

fn header(r: &mut P<'_>, p: &mut Preview) -> Option<()> {
    let (maj, min) = (r.u8()?, r.u8()?);
    let game = r.u16()?;
    let time = r.u64()?;
    let source = r.wstr()?;
    let user = r.wstr()?;
    let machine = r.wstr()?;
    let gname = match game {
        1 => "Skyrim",
        2 => "Fallout 4",
        3 => "Fallout 76",
        4 => "Starfield",
        _ => "неизвестная",
    };
    p.fact("Игра", gname);
    p.fact("Версия PEX", format!("{maj}.{min}"));
    p.fact("Исходник", &source);
    p.fact("Собрал", format!("{user} @ {machine}"));
    p.fact("Собран", date(time));
    let ns = r.u16()?;
    for _ in 0..ns {
        let s = r.wstr()?;
        r.strings.push(s);
    }
    if r.u8()? != 0 {
        r.u64()?;
        let nf = r.u16()?;
        for _ in 0..nf {
            r.u16()?;
            r.u16()?;
            r.u16()?;
            r.u8()?;
            let n = r.u16()?;
            r.c.skip(n as usize * 2)?;
        }
    }
    let nu = r.u16()?;
    for _ in 0..nu {
        r.u16()?;
        r.u8()?;
    }
    let no = r.u16()?;
    for _ in 0..no.min(4) {
        let name = r.s()?;
        let _size = r.u32()?;
        let parent = r.s()?;
        p.fact("Скрипт", &name);
        p.format = format!("PEX · {gname} · {name}");
        if !parent.is_empty() {
            p.fact("Наследует", &parent);
            p.format.push_str(&format!(" extends {parent}"));
            p.link(&format!("scripts/{parent}.pex"));
        }
        p.link(&format!("scripts/source/{name}.psc"));
        if game != 1 {
            p.note("функции разбираются только у скриптов Skyrim");
            return Some(());
        }
        let _doc = r.s()?;
        let _uf = r.u32()?;
        let auto = r.s()?;
        if !auto.is_empty() {
            p.fact("Начальное состояние", auto);
        }
        let nv = r.u16()?;
        let mut vars = Vec::new();
        for _ in 0..nv {
            let n = r.s()?;
            let t = r.s()?;
            r.u32()?;
            let v = r.value()?;
            vars.push(if v == "None" {
                format!("{t} {n}")
            } else {
                format!("{t} {n} = {v}")
            });
        }
        let np = r.u16()?;
        let mut props = Vec::new();
        for _ in 0..np {
            let n = r.s()?;
            let t = r.s()?;
            let _doc = r.s()?;
            r.u32()?;
            let flags = r.u8()?;
            if flags & 4 != 0 {
                r.s()?;
                props.push(format!("{t} {n} (auto)"));
            } else {
                if flags & 1 != 0 {
                    r.function("get")?;
                }
                if flags & 2 != 0 {
                    r.function("set")?;
                }
                props.push(format!("{t} {n}"));
            }
        }
        let nst = r.u16()?;
        let mut funcs = Vec::new();
        for _ in 0..nst {
            let state = r.s()?;
            let nf = r.u16()?;
            for _ in 0..nf {
                let fname = r.s()?;
                let sig = r.function(&fname)?;
                funcs.push(if state.is_empty() {
                    sig
                } else {
                    format!("[{state}] {sig}")
                });
            }
        }
        p.fact("Свойств", props.len());
        p.fact("Функций", funcs.len());
        p.section("Свойства", props);
        p.section("Переменные", vars);
        p.section("Функции и события", funcs);
    }
    Some(())
}

/// Unix seconds → `YYYY-MM-DD hh:mm` (UTC).
fn date(t: u64) -> String {
    let days = (t / 86_400) as i64;
    let secs = t % 86_400;
    // civil from days (Howard Hinnant)
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = if m <= 2 { y + 1 } else { y };
    format!("{y:04}-{m:02}-{d:02} {:02}:{:02}", secs / 3600, secs % 3600 / 60)
}

#[cfg(test)]
mod tests {
    use super::super::{Kind, inspect};

    fn w(b: &mut Vec<u8>, s: &str) {
        b.extend_from_slice(&(s.len() as u16).to_be_bytes());
        b.extend_from_slice(s.as_bytes());
    }

    #[test]
    fn a_skyrim_script_says_its_parent_properties_and_functions() {
        let mut b = vec![0xFA, 0x57, 0xC0, 0xDE, 3, 2];
        b.extend_from_slice(&1u16.to_be_bytes());
        b.extend_from_slice(&1_700_000_000u64.to_be_bytes());
        w(&mut b, "MyQuest.psc");
        w(&mut b, "me");
        w(&mut b, "pc");
        // strings: 0 MyQuest, 1 Quest, 2 "", 3 Count, 4 Int, 5 OnInit, 6 None
        let strs = ["MyQuest", "Quest", "", "Count", "Int", "OnInit", "None"];
        b.extend_from_slice(&(strs.len() as u16).to_be_bytes());
        for s in strs {
            w(&mut b, s);
        }
        b.push(0); // no debug info
        b.extend_from_slice(&0u16.to_be_bytes()); // user flags
        b.extend_from_slice(&1u16.to_be_bytes()); // objects
        let s = |b: &mut Vec<u8>, i: u16| b.extend_from_slice(&i.to_be_bytes());
        s(&mut b, 0);
        b.extend_from_slice(&0u32.to_be_bytes());
        s(&mut b, 1); // parent
        s(&mut b, 2); // doc
        b.extend_from_slice(&0u32.to_be_bytes());
        s(&mut b, 2); // auto state
        s(&mut b, 0); // variables
        s(&mut b, 1); // properties
        s(&mut b, 3);
        s(&mut b, 4);
        s(&mut b, 2);
        b.extend_from_slice(&0u32.to_be_bytes());
        b.push(4);
        s(&mut b, 3); // auto var name
        s(&mut b, 1); // states
        s(&mut b, 2); // "" state
        s(&mut b, 1); // functions
        s(&mut b, 5); // OnInit
        s(&mut b, 6); // return None
        s(&mut b, 2);
        b.extend_from_slice(&0u32.to_be_bytes());
        b.push(0);
        s(&mut b, 0); // params
        s(&mut b, 0); // locals
        s(&mut b, 1); // one instruction: return None
        b.push(26);
        b.push(0);
        let p = inspect("scripts/myquest.pex", &b);
        assert_eq!(p.kind, Kind::Script);
        assert!(p.notes.is_empty(), "{:?}", p.notes);
        assert_eq!(p.format, "PEX · Skyrim · MyQuest extends Quest");
        assert_eq!(p.sections[0].lines, ["Int Count (auto)"]);
        assert_eq!(
            p.sections
                .iter()
                .find(|s| s.title == "Функции и события")
                .unwrap()
                .lines,
            ["OnInit()"]
        );
        assert_eq!(p.links[0], "scripts/quest.pex");
        assert!(
            p.facts
                .iter()
                .any(|(k, v)| k == "Собран" && v == "2023-11-14 22:13")
        );
    }
}
