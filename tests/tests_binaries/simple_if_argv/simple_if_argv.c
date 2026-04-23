// Header file for input output functions
#include <stdio.h>
#include <stdlib.h>

// Main function: entry point for execution
int main(int argc, char *argv[]) {
    // This condition depends on argv so decompilation should retrieve it
    if (atoi(argv[1]) > 18) {
        printf("Input is greater than 18\n");
    } else {
        printf("Input is egal or lower than 18\n");
    }
}
