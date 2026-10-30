// Command modlisttool runs the ModlistTool engine with its web UI on localhost.
package main

import (
	"flag"
	"fmt"
	"io/fs"
	"log"
	"net"
	"net/http"
	"os"
	"path/filepath"
	"time"

	"modlisttool/internal/engine"
	"modlisttool/internal/server"
	"modlisttool/internal/synth"
	"modlisttool/web"
)

var version = "0.1.0-dev"

func main() {
	addr := flag.String("addr", "127.0.0.1:7317", "адрес HTTP")
	noBrowser := flag.Bool("no-browser", false, "не открывать браузер")
	synthDir := flag.String("synth", "", "сгенерировать синтетический MO2-инстанс в папке и выйти")
	synthMods := flag.Int("synth-mods", 400, "модов в синтетике")
	synthFiles := flag.Int("synth-files", 300, "файлов на мод (среднее)")
	open := flag.String("open", "", "сразу открыть инстанс (ModOrganizer.ini или папка)")
	extra := flag.String("extra", "", "дополнительная папка модов")
	scanNow := flag.Bool("scan", false, "сразу запустить пробег после -open")
	flag.Parse()

	if *synthDir != "" {
		t := time.Now()
		p, err := synth.Instance(*synthDir, *synthMods, *synthFiles, 42)
		if err != nil {
			log.Fatal(err)
		}
		fmt.Printf("синтетика: %s (%.1fs)\n", p, time.Since(t).Seconds())
		return
	}

	exe, _ := os.Executable()
	srv := &server.Server{E: engine.New(), CfgPath: filepath.Join(filepath.Dir(exe), "modlisttool.json"), Version: version}
	srv.LoadConfig()
	if sub, err := fs.Sub(web.Dist, "dist"); err == nil {
		srv.Static = sub
	}
	if *open != "" {
		var ex []string
		if *extra != "" {
			ex = []string{*extra}
		}
		if err := srv.E.Open(*open, ex, ""); err != nil {
			log.Fatal(err)
		}
		if *scanNow {
			_ = srv.E.StartScan(engine.DefaultScanOptions())
		}
	} else if srv.Cfg.Instance != "" {
		if err := srv.E.Open(srv.Cfg.Instance, srv.Cfg.Extra, srv.Cfg.Profile); err != nil {
			log.Printf("не удалось открыть прошлый инстанс: %v", err)
		}
	}
	ln, err := net.Listen("tcp", *addr)
	if err != nil {
		log.Fatalf("порт занят? %v", err)
	}
	url := "http://" + ln.Addr().String() + "/"
	fmt.Printf("ModlistTool %s — %s\nЗакрой это окно, чтобы остановить.\n", version, url)
	if !*noBrowser {
		go server.OpenBrowser(url)
	}
	log.Fatal(http.Serve(ln, srv.Handler()))
}
