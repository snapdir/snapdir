// smoke prints the snapdir version via the CGo binding.
package main

import (
	"fmt"

	snapdir "github.com/snapdir/snapdir/bindings/go"
)

func main() {
	fmt.Println(snapdir.Version())
}
