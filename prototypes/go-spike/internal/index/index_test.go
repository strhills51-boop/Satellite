package index

import (
	"os"
	"runtime"
	"strconv"
	"testing"
	"time"

	"modlisttool/internal/scan"
	"modlisttool/internal/synth"
)

func TestSmallConflicts(t *testing.T) {
	mk := func(name string, files ...string) *scan.Mod {
		m := &scan.Mod{Name: name}
		for _, f := range files {
			dir, base := "", f
			for i := len(f) - 1; i >= 0; i-- {
				if f[i] == '/' {
					dir, base = f[:i], f[i+1:]
					break
				}
			}
			m.Dirs = append(m.Dirs, scan.Dir{Rel: dir, Files: []scan.File{{Name: base, Size: 10}}})
			m.Metrics.Files++
		}
		return m
	}
	mods := []*scan.Mod{
		mk("A", "textures/a.dds", "textures/b.dds", "meshes/x.nif"),
		mk("B", "Textures/A.dds", "textures/c.dds"),
		mk("C", "textures/a.dds", "textures/b.dds", "textures/c.dds"),
		mk("D", "other/only.txt"),
		mk("E", "meshes/x.nif"),
	}
	// archive in D that conflicts with loose meshes/x.nif: loose must win regardless of priority
	mods[3].Archives = []scan.Archive{{Name: "D.bsa", Entries: nil}}
	tb := Build(mods)
	prio := []int32{0, 1, 2, 3, -1} // E disabled
	c := Compute(tb, prio)
	if c.T.NumPaths() != 5 {
		t.Fatalf("paths=%d", c.T.NumPaths())
	}
	// a.dds: C>B>A ; b.dds: C>A ; c.dds: C>B ; x.nif: only A (E disabled)
	if len(c.Chains) != 3 {
		for _, ch := range c.Chains {
			t.Log(ch)
		}
		t.Fatalf("chains=%d", len(c.Chains))
	}
	if c.Stats[2].Win != 3 || c.Stats[0].Lose != 2 || c.Stats[1].Mid != 1 {
		t.Fatalf("stats %+v", c.Stats)
	}
	if !(c.Stats[0].Unique == 1 && !c.Stats[0].Full) {
		t.Fatalf("A %+v", c.Stats[0])
	}
	if len(c.Contexts) != 1 || len(c.Contexts[0].Mods) != 3 {
		t.Fatalf("contexts %+v", c.Contexts)
	}
	// B fully overridden? B wins nothing, unique nothing -> X
	if !c.Stats[1].Full {
		t.Fatalf("B should be X: %+v", c.Stats[1])
	}
	// enable E above A: x.nif conflict
	c2 := Compute(tb, []int32{0, 1, 2, 3, 4})
	if c2.Stats[4].Win != 1 || c2.Stats[0].Unique != 0 {
		t.Fatalf("E %+v A %+v", c2.Stats[4], c2.Stats[0])
	}
}

// go test ./internal/index -run Bench -v  (BENCH_MODS / BENCH_FILES env)
func TestBenchSynthetic(t *testing.T) {
	if os.Getenv("BENCH") == "" {
		t.Skip("set BENCH=1")
	}
	n, _ := strconv.Atoi(os.Getenv("BENCH_MODS"))
	fpm, _ := strconv.Atoi(os.Getenv("BENCH_FILES"))
	if n == 0 {
		n = 3000
	}
	if fpm == 0 {
		fpm = 1000
	}
	t0 := time.Now()
	mods := synth.Mods(n, fpm, 0.3, 1)
	total := 0
	for _, m := range mods {
		total += m.Metrics.Files
	}
	t.Logf("synth: mods=%d files=%d in %v", n, total, time.Since(t0))
	mem("after synth", t)
	t1 := time.Now()
	tb := Build(mods)
	t.Logf("build: paths=%d providers=%d dirs=%d names=%d in %v", tb.NumPaths(), len(tb.Key), len(tb.In.DirParent), len(tb.In.Names), time.Since(t1))
	mods = nil
	runtime.GC()
	mem("table only", t)
	prio := make([]int32, n)
	for i := range prio {
		prio[i] = int32(i)
	}
	for round := 0; round < 3; round++ {
		t2 := time.Now()
		c := Compute(tb, prio)
		t.Logf("compute#%d: conflictPaths=%d chains=%d contexts=%d in %v", round, c.ConflictPaths, len(c.Chains), len(c.Contexts), time.Since(t2))
		// reorder: move a block of mods
		for i := range prio {
			prio[i] = int32((i*7919 + round) % n)
		}
	}
	mem("after compute", t)
}

func mem(label string, t *testing.T) {
	var ms runtime.MemStats
	runtime.GC()
	runtime.ReadMemStats(&ms)
	t.Logf("%s: heap=%d MiB sys=%d MiB", label, ms.HeapAlloc>>20, ms.Sys>>20)
}
