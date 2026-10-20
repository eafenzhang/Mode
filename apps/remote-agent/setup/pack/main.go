package main

import (
	"archive/zip"
	"bytes"
	"encoding/binary"
	"flag"
	"fmt"
	"io/ioutil"
	"os"
)

const footerMagic = "MODEAGNT"

func addFile(writer *zip.Writer, name, path string) error {
	entry, err := writer.Create(name)
	if err != nil {
		return err
	}
	data, err := ioutil.ReadFile(path)
	if err != nil {
		return err
	}
	_, err = entry.Write(data)
	return err
}

func main() {
	bootstrap := flag.String("bootstrap", "", "32-bit bootstrap executable")
	x86 := flag.String("x86", "", "32-bit agent executable")
	x64 := flag.String("x64", "", "64-bit agent executable")
	output := flag.String("out", "", "single installer output path")
	flag.Parse()
	if *bootstrap == "" || *x86 == "" || *x64 == "" || *output == "" {
		fmt.Fprintln(os.Stderr, "usage: pack -bootstrap <exe> -x86 <exe> -x64 <exe> -out <exe>")
		os.Exit(2)
	}
	var archive bytes.Buffer
	writer := zip.NewWriter(&archive)
	if err := addFile(writer, "mode-agent-x86.exe", *x86); err != nil {
		fmt.Fprintln(os.Stderr, "pack x86:", err)
		os.Exit(1)
	}
	if err := addFile(writer, "mode-agent-x64.exe", *x64); err != nil {
		fmt.Fprintln(os.Stderr, "pack x64:", err)
		os.Exit(1)
	}
	if err := writer.Close(); err != nil {
		fmt.Fprintln(os.Stderr, "finish zip:", err)
		os.Exit(1)
	}
	bootstrapData, err := ioutil.ReadFile(*bootstrap)
	if err != nil {
		fmt.Fprintln(os.Stderr, "read bootstrap:", err)
		os.Exit(1)
	}
	footer := make([]byte, 24)
	copy(footer, footerMagic)
	binary.LittleEndian.PutUint64(footer[8:16], uint64(len(bootstrapData)))
	binary.LittleEndian.PutUint64(footer[16:24], uint64(archive.Len()))
	outputData := make([]byte, 0, len(bootstrapData)+archive.Len()+len(footer))
	outputData = append(outputData, bootstrapData...)
	outputData = append(outputData, archive.Bytes()...)
	outputData = append(outputData, footer...)
	if err := ioutil.WriteFile(*output, outputData, 0755); err != nil {
		fmt.Fprintln(os.Stderr, "write installer:", err)
		os.Exit(1)
	}
	fmt.Printf("installer=%s bytes=%d\n", *output, len(outputData))
}
