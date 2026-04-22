// Header file for input output functions
#include <stdio.h>

// Main function: entry point for execution
int main() {
    
    int arr[] = {2, 4, 8, 12, 16, 18};
    int n = sizeof(arr)/sizeof(arr[0]);

    // Printing array elements
    for (int i = 0; i < n; i++) {
        printf("%d ", arr[i]);
    }
    
    // Simple return, but important to chack the pipeline restitution
    printf("\n");
}
