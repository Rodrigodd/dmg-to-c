module combinational_procedure(
    input logic a, b,
    output logic y
);
    always_comb y = a & b;
endmodule
