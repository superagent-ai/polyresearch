;name paper_6599
;strategy paper replicator with distance 6599, cnt=20

dist    EQU 6599

        ORG start

ptr     DAT.F  #0, #0
start   MOV.AB #20, $ptr
loop    MOV.I  @ptr, <dest
        DJN.B  $loop, $ptr
        SPL.B  @dest, $0
        ADD.AB #dist, $dest
        JMZ.B  $start, $ptr
dest    DAT.F  #0, #dist
