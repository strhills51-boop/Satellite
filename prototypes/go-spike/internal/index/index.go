// Package index turns per-mod scans into a compact, column-oriented virtual file system
// and computes overwrite stacks, exact chains and contexts.
//
// Everything is integer-addressed: a path is a uint32, a mod is a uint32. Nothing per-path is
// stored as a string except in the interner (directory names and file names are shared).
package index

import (
	"runtime"
	"slices"
	"strings"
	"sync"

	"modlisttool/internal/scan"
)

// ---------------------------------------------------------------- interner

// Interner deduplicates path segments into a directory tree.
type Interner struct {
	nameIdx   map[string]uint32 // lowercased segment -> id
	Names     []string          // id -> display (first seen casing)
	dirIdx    map[uint64]uint32 // parent<<32|name -> dir id
	DirParent []uint32
	DirName   []uint32
}

// NewInterner creates an interner with the root directory (id 0).
func NewInterner() *Interner {
	in := &Interner{nameIdx: map[string]uint32{"": 0}, Names: []string{""}, dirIdx: map[uint64]uint32{}}
	in.DirParent = []uint32{0}
	in.DirName = []uint32{0}
	return in
}

// Name interns one segment.
func (in *Interner) Name(s string) uint32 {
	ls := strings.ToLower(s)
	if id, ok := in.nameIdx[ls]; ok {
		return id
	}
	id := uint32(len(in.Names))
	in.nameIdx[ls] = id
	in.Names = append(in.Names, s)
	return id
}

// LookupName returns a segment id without inserting.
func (in *Interner) LookupName(s string) (uint32, bool) {
	id, ok := in.nameIdx[strings.ToLower(s)]
	return id, ok
}

// Dir interns a forward-slash relative directory path.
func (in *Interner) Dir(rel string) uint32 {
	cur := uint32(0)
	if rel == "" {
		return 0
	}
	for len(rel) > 0 {
		seg := rel
		if i := strings.IndexByte(rel, '/'); i >= 0 {
			seg, rel = rel[:i], rel[i+1:]
		} else {
			rel = ""
		}
		if seg == "" {
			continue
		}
		n := in.Name(seg)
		k := uint64(cur)<<32 | uint64(n)
		id, ok := in.dirIdx[k]
		if !ok {
			id = uint32(len(in.DirParent))
			in.dirIdx[k] = id
			in.DirParent = append(in.DirParent, cur)
			in.DirName = append(in.DirName, n)
		}
		cur = id
	}
	return cur
}

// DirPath reconstructs a directory path.
func (in *Interner) DirPath(d uint32) string {
	if d == 0 {
		return ""
	}
	var parts []string
	for d != 0 {
		parts = append(parts, in.Names[in.DirName[d]])
		d = in.DirParent[d]
	}
	slices.Reverse(parts)
	return strings.Join(parts, "/")
}

// KeyPath reconstructs a file path from a provider key.
func (in *Interner) KeyPath(k uint64) string {
	dir := in.DirPath(uint32(k >> 32))
	name := in.Names[uint32(k)]
	if dir == "" {
		return name
	}
	return dir + "/" + name
}

// ---------------------------------------------------------------- static table

// Source kinds.
const (
	SrcLoose uint8 = 0
	SrcArch  uint8 = 1
)

// ArchiveRef identifies an archive that contributes virtual paths.
type ArchiveRef struct {
	Mod     uint32 `json:"mod"`
	Name    string `json:"name"`
	Kind    string `json:"kind"`
	Entries int    `json:"entries"`
	Error   string `json:"error,omitempty"`
}

// Table is the immutable result of one scan: all providers sorted by path.
type Table struct {
	In       *Interner
	Mods     []string // mod id -> name (as in scan jobs)
	Archives []ArchiveRef

	// provider columns, sorted by Key
	Key  []uint64
	Mod  []uint32
	Size []uint64
	Arch []uint32 // 0 loose, else archive index + 1
	VRAM []uint32 // KiB, 0 unknown

	// path groups: providers [PathOff[p], PathOff[p+1]) share Key
	PathKey []uint64
	PathOff []uint32

	// mod -> providers (indices into provider columns), CSR
	ModOff  []uint32
	ModProv []uint32
}

// NumPaths returns the number of distinct virtual paths.
func (t *Table) NumPaths() int { return len(t.PathKey) }

// PathString returns the display path of a path group.
func (t *Table) PathString(p uint32) string { return t.In.KeyPath(t.PathKey[p]) }

// Build interns all scans into a Table. Interning is single-threaded (map writes),
// column sort and grouping are parallel.
func Build(mods []*scan.Mod) *Table {
	t := &Table{In: NewInterner()}
	total := 0
	for _, m := range mods {
		if m == nil {
			continue
		}
		total += m.Metrics.Files
		for _, a := range m.Archives {
			total += len(a.Entries)
		}
	}
	key := make([]uint64, 0, total)
	mod := make([]uint32, 0, total)
	size := make([]uint64, 0, total)
	arch := make([]uint32, 0, total)
	vram := make([]uint32, 0, total)
	for mi, m := range mods {
		name := ""
		if m != nil {
			name = m.Name
		}
		t.Mods = append(t.Mods, name)
		if m == nil {
			continue
		}
		for _, d := range m.Dirs {
			dir := uint64(t.In.Dir(d.Rel)) << 32
			for _, f := range d.Files {
				key = append(key, dir|uint64(t.In.Name(f.Name)))
				mod = append(mod, uint32(mi))
				size = append(size, uint64(f.Size))
				arch = append(arch, 0)
				vram = append(vram, f.VRAMKiB)
			}
		}
		for _, a := range m.Archives {
			t.Archives = append(t.Archives, ArchiveRef{Mod: uint32(mi), Name: a.Name, Kind: a.Kind, Entries: len(a.Entries), Error: a.Error})
			aid := uint32(len(t.Archives))
			for _, e := range a.Entries {
				dir, file := "", e.Path
				if i := strings.LastIndexByte(e.Path, '/'); i >= 0 {
					dir, file = e.Path[:i], e.Path[i+1:]
				}
				key = append(key, uint64(t.In.Dir(dir))<<32|uint64(t.In.Name(file)))
				mod = append(mod, uint32(mi))
				size = append(size, e.Size)
				arch = append(arch, aid)
				vram = append(vram, uint32(min(e.VRAM/1024, 1<<32-1)))
			}
		}
	}
	n := len(key)
	perm := sortedPerm(key)
	t.Key = make([]uint64, n)
	t.Mod = make([]uint32, n)
	t.Size = make([]uint64, n)
	t.Arch = make([]uint32, n)
	t.VRAM = make([]uint32, n)
	parallelFor(n, func(lo, hi int) {
		for i := lo; i < hi; i++ {
			j := perm[i]
			t.Key[i], t.Mod[i], t.Size[i], t.Arch[i], t.VRAM[i] = key[j], mod[j], size[j], arch[j], vram[j]
		}
	})
	key, mod, size, arch, vram, perm = nil, nil, nil, nil, nil, nil
	// path groups
	for i := 0; i < n; i++ {
		if i == 0 || t.Key[i] != t.Key[i-1] {
			t.PathKey = append(t.PathKey, t.Key[i])
			t.PathOff = append(t.PathOff, uint32(i))
		}
	}
	t.PathOff = append(t.PathOff, uint32(n))
	// mod CSR (counting sort, stable -> providers per mod are in path order)
	t.ModOff = make([]uint32, len(t.Mods)+1)
	for _, m := range t.Mod {
		t.ModOff[m+1]++
	}
	for i := 1; i < len(t.ModOff); i++ {
		t.ModOff[i] += t.ModOff[i-1]
	}
	t.ModProv = make([]uint32, n)
	fill := slices.Clone(t.ModOff[:len(t.Mods)])
	for i, m := range t.Mod {
		t.ModProv[fill[m]] = uint32(i)
		fill[m]++
	}
	return t
}

// PathOfProvider finds the path group for a provider index (binary search).
func (t *Table) PathOfProvider(i uint32) uint32 {
	p, _ := slices.BinarySearch(t.PathOff, i+1)
	return uint32(p - 1)
}

// sortedPerm returns indices sorted by key (LSD radix, 16-bit digits).
func sortedPerm(key []uint64) []uint32 {
	n := len(key)
	a := make([]uint32, n)
	b := make([]uint32, n)
	for i := range a {
		a[i] = uint32(i)
	}
	var cnt [1 << 16]uint32
	for shift := uint(0); shift < 64; shift += 16 {
		// skip a pass when all digits are equal
		first := uint16(0)
		if n > 0 {
			first = uint16(key[a[0]] >> shift)
		}
		same := true
		clear(cnt[:])
		for _, i := range a {
			d := uint16(key[i] >> shift)
			cnt[d]++
			if d != first {
				same = false
			}
		}
		if same {
			continue
		}
		sum := uint32(0)
		for d := range cnt {
			c := cnt[d]
			cnt[d] = sum
			sum += c
		}
		for _, i := range a {
			d := uint16(key[i] >> shift)
			b[cnt[d]] = i
			cnt[d]++
		}
		a, b = b, a
	}
	return a
}

func parallelFor(n int, fn func(lo, hi int)) {
	w := runtime.GOMAXPROCS(0)
	if n < 1<<14 || w == 1 {
		fn(0, n)
		return
	}
	chunk := (n + w - 1) / w
	var wg sync.WaitGroup
	for lo := 0; lo < n; lo += chunk {
		hi := min(n, lo+chunk)
		wg.Add(1)
		go func() { defer wg.Done(); fn(lo, hi) }()
	}
	wg.Wait()
}
