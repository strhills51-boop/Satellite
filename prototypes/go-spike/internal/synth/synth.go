// Package synth generates synthetic modlists for tests and benchmarks.
package synth

import (
	"encoding/binary"
	"fmt"
	"math/rand"
	"os"
	"path/filepath"
	"strconv"
	"strings"

	"modlisttool/internal/scan"
)

var areas = []string{
	"textures/actors/character/female", "textures/actors/character/male", "textures/armor/steel",
	"textures/armor/daedric", "textures/landscape", "textures/landscape/trees", "textures/architecture/whiterun",
	"textures/architecture/windhelm", "textures/clutter", "textures/effects", "textures/terrain/lods",
	"meshes/actors/character/character assets", "meshes/armor/steel", "meshes/landscape/trees",
	"meshes/architecture/whiterun", "meshes/clutter", "meshes/actors/character/animations",
	"scripts", "interface", "sound/fx", "seq", "skse/plugins", "textures/sky", "meshes/weapons",
	"textures/weapons", "lodsettings", "grass", "meshes/terrain/tamriel", "textures/terrain/tamriel",
}

var exts = []string{".dds", ".dds", ".dds", ".dds", ".nif", ".nif", ".pex", ".hkx", ".wav", ".xml", ".json", ".ini"}

// Mods returns in-memory scans: n mods, ~filesPerMod each, overlap controls conflict density.
func Mods(n, filesPerMod int, overlap float64, seed int64) []*scan.Mod {
	r := rand.New(rand.NewSource(seed))
	out := make([]*scan.Mod, n)
	for i := range out {
		m := &scan.Mod{Name: fmt.Sprintf("Mod %05d", i)}
		// skewed sizes: a few huge mods
		cnt := filesPerMod / 4
		switch x := r.Float64(); {
		case x < 0.02:
			cnt = filesPerMod * 40
		case x < 0.15:
			cnt = filesPerMod * 4
		case x < 0.5:
			cnt = filesPerMod
		}
		dirs := map[string][]scan.File{}
		home := r.Intn(len(areas))
		for f := 0; f < cnt; f++ {
			area := areas[home]
			if r.Float64() < 0.3 {
				area = areas[r.Intn(len(areas))]
			}
			var name string
			if r.Float64() < overlap { // shared vocabulary -> conflicts
				name = fmt.Sprintf("shared_%04d%s", r.Intn(300), exts[r.Intn(len(exts))])
			} else {
				name = fmt.Sprintf("m%05d_%06d%s", i, f, exts[r.Intn(len(exts))])
			}
			sub := fmt.Sprintf("%s/set%02d", area, r.Intn(4))
			dirs[sub] = append(dirs[sub], scan.File{Name: name, Size: int64(r.Intn(4 << 20)), VRAMKiB: uint32(r.Intn(5000))})
		}
		for d, fs := range dirs {
			m.Dirs = append(m.Dirs, scan.Dir{Rel: d, Files: dedupe(fs)})
			m.Metrics.Files += len(m.Dirs[len(m.Dirs)-1].Files)
		}
		out[i] = m
	}
	return out
}

func dedupe(fs []scan.File) []scan.File {
	seen := map[string]bool{}
	out := fs[:0]
	for _, f := range fs {
		k := strings.ToLower(f.Name)
		if !seen[k] {
			seen[k] = true
			out = append(out, f)
		}
	}
	return out
}

// Instance writes a synthetic MO2 instance to disk (empty loose files, real headers for
// plugins/DDS/BSA so parsers are exercised). Returns the ModOrganizer.ini path.
func Instance(root string, nMods, filesPerMod int, seed int64) (string, error) {
	r := rand.New(rand.NewSource(seed))
	modsDir := filepath.Join(root, "mods")
	mods2 := filepath.Join(root, "mods2")
	prof := filepath.Join(root, "profiles", "Default")
	for _, d := range []string{modsDir, mods2, prof, filepath.Join(root, "overwrite", "skse", "plugins")} {
		if err := os.MkdirAll(d, 0o755); err != nil {
			return "", err
		}
	}
	ini := "[General]\r\ngameName=Skyrim Special Edition\r\nselected_profile=@ByteArray(Default)\r\n[Settings]\r\n"
	if err := os.WriteFile(filepath.Join(root, "ModOrganizer.ini"), []byte(ini), 0o644); err != nil {
		return "", err
	}
	os.WriteFile(filepath.Join(root, "overwrite", "skse", "plugins", "overwrite_test.ini"), []byte("x"), 0o644)
	var lines []string // display order (lowest priority first)
	var plugins []string
	colors := []string{`\0\0\0\x43\x1\xff\xff\x80\x80\x30\x30\xd0\xd0\0\0`, `\0\0\0\x43\x1\xff\xff\x30\x30\xa0\xa0\x40\x40\0\0`, `\0\0\0\x43\x1\xff\xff\xc0\xc0\x50\x50\x50\x50\0\0`}
	sepNames := []string{"Core", "Fixes", "UI", "Textures - Landscape", "Textures - Architecture", "Meshes", "Animation", "Armor", "Weapons", "Gameplay", "LOD", "Patches"}
	perSep := max(1, nMods/len(sepNames))
	modi := 0
	for s, sn := range sepNames {
		sep := sn + "_separator"
		sd := filepath.Join(modsDir, sep)
		os.MkdirAll(sd, 0o755)
		os.WriteFile(filepath.Join(sd, "meta.ini"), []byte("[General]\r\ncolor=@Variant("+colors[s%len(colors)]+")\r\n"), 0o644)
		lines = append(lines, "-"+sep)
		cnt := perSep
		if s == len(sepNames)-1 {
			cnt = nMods - modi
		}
		for k := 0; k < cnt && modi < nMods; k++ {
			name := fmt.Sprintf("%s mod %04d", sn, modi)
			base := modsDir
			if modi%5 == 4 {
				base = mods2 // second mods folder
			}
			md := filepath.Join(base, name)
			if err := writeMod(r, md, modi, s, filesPerMod); err != nil {
				return "", err
			}
			if modi%7 == 0 {
				p := fmt.Sprintf("Synth%04d.esp", modi)
				writePlugin(filepath.Join(md, p), modi%21 == 0, []string{"Skyrim.esm"})
				plugins = append(plugins, p)
				if modi%14 == 0 {
					writeBSA(filepath.Join(md, strings.TrimSuffix(p, ".esp")+".bsa"), r, s)
				}
			}
			os.WriteFile(filepath.Join(md, "meta.ini"), []byte(fmt.Sprintf("[General]\r\nmodid=%d\r\nversion=1.%d\r\n", 1000+modi, modi%9)), 0o644)
			sign := "+"
			if r.Float64() < 0.08 {
				sign = "-"
			}
			lines = append(lines, sign+name)
			modi++
		}
	}
	lines = append(lines, "*DLC: Dawnguard")
	// modlist.txt is highest priority first
	var b strings.Builder
	b.WriteString("# This file was automatically generated by Mod Organizer.\r\n")
	for i := len(lines) - 1; i >= 0; i-- {
		b.WriteString(lines[i] + "\r\n")
	}
	os.WriteFile(filepath.Join(prof, "modlist.txt"), []byte(b.String()), 0o644)
	var pl, lo strings.Builder
	pl.WriteString("# This file was automatically generated by Mod Organizer.\r\n")
	lo.WriteString("Skyrim.esm\r\nUpdate.esm\r\n")
	for _, p := range plugins {
		pl.WriteString("*" + p + "\r\n")
		lo.WriteString(p + "\r\n")
	}
	os.WriteFile(filepath.Join(prof, "plugins.txt"), []byte(pl.String()), 0o644)
	os.WriteFile(filepath.Join(prof, "loadorder.txt"), []byte(lo.String()), 0o644)
	return filepath.Join(root, "ModOrganizer.ini"), nil
}

func writeMod(r *rand.Rand, dir string, i, sep, filesPerMod int) error {
	cnt := filesPerMod/2 + r.Intn(filesPerMod)
	if r.Float64() < 0.03 {
		cnt *= 20
	}
	home := areas[(sep*3+r.Intn(3))%len(areas)]
	for f := 0; f < cnt; f++ {
		area := home
		if r.Float64() < 0.25 {
			area = areas[r.Intn(len(areas))]
		}
		ext := exts[r.Intn(len(exts))]
		var name string
		if r.Float64() < 0.35 {
			name = fmt.Sprintf("shared_%03d%s", r.Intn(sharedRange()), ext)
		} else {
			name = fmt.Sprintf("m%04d_%05d%s", i, f, ext)
		}
		d := filepath.Join(dir, filepath.FromSlash(fmt.Sprintf("%s/set%d", area, r.Intn(sharedSets()))))
		if err := os.MkdirAll(d, 0o755); err != nil {
			return err
		}
		var data []byte
		if ext == ".dds" {
			data = ddsHeader(uint32(256<<r.Intn(4)), uint32(256<<r.Intn(4)))
		}
		if err := os.WriteFile(filepath.Join(d, name), data, 0o644); err != nil {
			return err
		}
	}
	return nil
}

func ddsHeader(w, h uint32) []byte {
	b := make([]byte, 128)
	copy(b, "DDS ")
	le := binary.LittleEndian
	le.PutUint32(b[4:], 124)
	le.PutUint32(b[12:], h)
	le.PutUint32(b[16:], w)
	le.PutUint32(b[28:], 1+uint32(bitsLen(max(w, h))))
	le.PutUint32(b[76:], 32)
	le.PutUint32(b[80:], 0x4)
	copy(b[84:], "DXT5")
	return b
}

func bitsLen(x uint32) int {
	n := 0
	for x > 1 {
		x >>= 1
		n++
	}
	return n
}

func writePlugin(path string, esm bool, masters []string) {
	le := binary.LittleEndian
	var body []byte
	sub := func(sig string, data []byte) {
		h := make([]byte, 6)
		copy(h, sig)
		le.PutUint16(h[4:], uint16(len(data)))
		body = append(body, h...)
		body = append(body, data...)
	}
	hedr := make([]byte, 12)
	le.PutUint32(hedr, 0x3FD9999A) // 1.7
	le.PutUint32(hedr[4:], 3)
	le.PutUint32(hedr[8:], 0x800)
	sub("HEDR", hedr)
	sub("CNAM", []byte("synth\x00"))
	for _, m := range masters {
		sub("MAST", append([]byte(m), 0))
		sub("DATA", make([]byte, 8))
	}
	hdr := make([]byte, 24)
	copy(hdr, "TES4")
	le.PutUint32(hdr[4:], uint32(len(body)))
	if esm {
		le.PutUint32(hdr[8:], 1)
	}
	out := append(hdr, body...)
	// one GRUP with two REFR records
	rec := func(sig string) []byte {
		h := make([]byte, 24+4)
		copy(h, sig)
		le.PutUint32(h[4:], 4)
		return h
	}
	refrs := append(rec("REFR"), rec("REFR")...)
	g := make([]byte, 24)
	copy(g, "GRUP")
	le.PutUint32(g[4:], uint32(24+len(refrs)))
	out = append(out, g...)
	out = append(out, refrs...)
	out = append(out, rec("NPC_")...)
	os.WriteFile(path, out, 0o644)
}

// writeBSA writes a minimal valid BSA v105 manifest (file data is empty).
func writeBSA(path string, r *rand.Rand, sep int) {
	le := binary.LittleEndian
	folders := map[string][]string{}
	for i := 0; i < 40; i++ {
		a := areas[(sep*3+i)%len(areas)] + fmt.Sprintf("/set%d", r.Intn(6))
		folders[a] = append(folders[a], fmt.Sprintf("shared_%03d%s", r.Intn(600), exts[r.Intn(len(exts))]))
	}
	var names []string
	for k := range folders {
		names = append(names, k)
	}
	var fileCount, dirLen, fileLen int
	for _, k := range names {
		folders[k] = dedupe2(folders[k])
		fileCount += len(folders[k])
		dirLen += len(k) + 1
		for _, f := range folders[k] {
			fileLen += len(f) + 1
		}
	}
	h := make([]byte, 36)
	copy(h, "BSA\x00")
	le.PutUint32(h[4:], 105)
	le.PutUint32(h[8:], 36)
	le.PutUint32(h[12:], 0x3)
	le.PutUint32(h[16:], uint32(len(names)))
	le.PutUint32(h[20:], uint32(fileCount))
	le.PutUint32(h[24:], uint32(dirLen))
	le.PutUint32(h[28:], uint32(fileLen))
	out := h
	for _, k := range names {
		rec := make([]byte, 24)
		le.PutUint32(rec[8:], uint32(len(folders[k])))
		out = append(out, rec...)
	}
	for _, k := range names {
		out = append(out, byte(len(k)+1))
		out = append(out, k...)
		out = append(out, 0)
		for range folders[k] {
			fr := make([]byte, 16)
			le.PutUint32(fr[8:], 1234)
			out = append(out, fr...)
		}
	}
	for _, k := range names {
		for _, f := range folders[k] {
			out = append(out, f...)
			out = append(out, 0)
		}
	}
	os.WriteFile(path, out, 0o644)
}

func dedupe2(s []string) []string {
	seen := map[string]bool{}
	var out []string
	for _, x := range s {
		if !seen[x] {
			seen[x] = true
			out = append(out, x)
		}
	}
	return out
}

func envInt(k string, def int) int {
	if v, err := strconv.Atoi(os.Getenv(k)); err == nil && v > 0 {
		return v
	}
	return def
}
func sharedRange() int { return envInt("SYNTH_SHARED", 600) }
func sharedSets() int  { return envInt("SYNTH_SETS", 6) }
