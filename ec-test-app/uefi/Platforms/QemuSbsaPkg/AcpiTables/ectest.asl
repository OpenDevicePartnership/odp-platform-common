#include "thermal.asl"

//
// EC Test interface to load KMDF driver and map methods
//
Device (ECT0) {
  Name (_HID, "ACPI1234")
  Name (_UID, 0x0)
  Name (_CCA, 0x0)

  Name (NEVT, 0x0) 
  Name (SEQN, 0x1) // Global sequence number used for RX/TX queue

  Method (_STA) {
    Store(0x8,TCNT)
    Store(0x100,TVER)
    Return (0xf)
  }

  Method(TEST, 0x0, NotSerialized) {  
    \_SB.SKIN._DSM(ToUUID("1f0849fc-a845-4fcf-865c-4101bf8e8d79"),0,0,0) // Get features
    \_SB.SKIN._DSM(ToUUID("1f0849fc-a845-4fcf-865c-4101bf8e8d79"),0,1, Package() {0x1234, 0x10000, 0x20000} )
    \_SB.THRM._DSM(ToUUID("07ff6382-e29a-47c9-ac87-e79dad71dd82"),0,1,0) // Read variable OnTemp
    Return(\_SB.THRM._DSM(ToUUID("d9b9b7f3-2a3e-4064-8841-cb13d317669e"),0,1,0x11112222)) // Write variable OnTemp
    //Return( \_SB.SKIN._TMP() )
  }

  // Shared memory regions and ASYNC implementation
  OperationRegion (SMTX, SystemMemory, 0x10060000000, 0x1000)
  // Store our actual request to shared memory TX buffer
  Field (SMTX, AnyAcc, NoLock, Preserve)
  {
    TVER, 16,
    TCNT, 16,
    TRS0, 32,  
    TB0, 64,
    TB1, 64,
    TB2, 64,
    TB3, 64,
    TB4, 64,
    TB5, 64,
    TB6, 64,
    TB7, 64,
    Offset(0x100),  // First Entry starts at 256 byte offset each entry is 256 bytes
    TE0, 2048,
    TE1, 2048,
    TE2, 2048,
    TE3, 2048,
    TE4, 2048,
    TE5, 2048,
    TE6, 2048,
    TE7, 2048,
  }

  // Shared memory region
  OperationRegion (SMRX, SystemMemory, 0x10060001000, 0x1000)
  // Store our actual request to shared memory TX buffer
  Field (SMRX, AnyAcc, NoLock, Preserve)
  {
    RVER, 16,
    RCNT, 16,
    RRS0, 32,  
    RB0, 64,
    RB1, 64,
    RB2, 64,
    RB3, 64,
    RB4, 64,
    RB5, 64,
    RB6, 64,
    RB7, 64,
    Offset(0x100),  // First Entry starts at 256 byte offset each entry is 256 bytes
    RE0, 2048,
    RE1, 2048,
    RE2, 2048,
    RE3, 2048,
    RE4, 2048,
    RE5, 2048,
    RE6, 2048,
    RE7, 2048,
  }

  // Allow multiple threads to wait for their SEQ packet at once
  // If supporting packet > 256 bytes need to modify to stitch together packet
  Method(RXDB, 0x1, Serialized) {
    Name(BUFF, Buffer(256){})

    Local0 = 0
    // Loop for 500ms looking for data
    While (Local0 < 100) {
      If(LEqual(And(RB0,0xFFFF),Arg0)) {
        CreateField(BUFF, 0, Multiply(And(ShiftRight(RB0,16),0xFFFF),8), XB0)
        Store(RE0,BUFF); Store(0,RB0); Return( XB0 )
      }
      If(LEqual(And(RB1,0xFFFF),Arg0)) {
        CreateField(BUFF, 0, Multiply(And(ShiftRight(RB1,16),0xFFFF),8), XB1)
        Store(RE1,BUFF); Store(0,RB1); Return( XB1 )
      }
      If(LEqual(And(RB2,0xFFFF),Arg0)) {
        CreateField(BUFF, 0, Multiply(And(ShiftRight(RB2,16),0xFFFF),8), XB2)
        Store(RE2,BUFF); Store(0,RB2); Return( XB2 )
      }
      If(LEqual(And(RB3,0xFFFF),Arg0)) {
        CreateField(BUFF, 0, Multiply(And(ShiftRight(RB3,16),0xFFFF),8), XB3)
        Store(RE3,BUFF); Store(0,RB3); Return( XB3 )
      }
      If(LEqual(And(RB4,0xFFFF),Arg0)) {
        CreateField(BUFF, 0, Multiply(And(ShiftRight(RB4,16),0xFFFF),8), XB4)
        Store(RE4,BUFF); Store(0,RB4); Return( XB4 )
      }
      If(LEqual(And(RB5,0xFFFF),Arg0)) {
        CreateField(BUFF, 0, Multiply(And(ShiftRight(RB5,16),0xFFFF),8), XB5)
        Store(RE5,BUFF); Store(0,RB5); Return( XB5 )
      }
      If(LEqual(And(RB6,0xFFFF),Arg0)) {
        CreateField(BUFF, 0, Multiply(And(ShiftRight(RB6,16),0xFFFF),8), XB6)
        Store(RE6,BUFF); Store(0,RB6); Return( XB6 )
      }
      If(LEqual(And(RB7,0xFFFF),Arg0)) {
        CreateField(BUFF, 0, Multiply(And(ShiftRight(RB7,16),0xFFFF),8), XB7)
        Store(RE7,BUFF); Store(0,RB7); Return( XB7 )
      }
      Sleep(5)
      Local0++
    }

    // If we get here didn't find a matching sequence number
    Return (Ones)
  }

  // Arg0 is buffer pointer
  // Arg1 is length of Data
  // Return Seq #
  Method(QTXB, 0x2, Serialized) {
      Name(TBX, 0x0)
      Store(Add(ShiftLeft(1,32),Add(ShiftLeft(Arg1,16),SEQN)),TBX)
      Increment(SEQN)
      
      Local0 = 0
      // Loop for 500ms looking for data
      While (Local0 < 100) {
        If(LEqual(And(TB0,0xFFFF),0x0)) {
          Store(TBX,TB0); Store(Arg0,TE0); Return( And(TBX,0xFFFF) )
        }
        If(LEqual(And(TB1,0xFFFF),0x0)) {
          Store(TBX,TB1); Store(Arg0,TE1); Return( And(TBX,0xFFFF) )
        }
        If(LEqual(And(TB2,0xFFFF),0x0)) {
          Store(TBX,TB2); Store(Arg0,TE2); Return( And(TBX,0xFFFF) )
        }
        If(LEqual(And(TB3,0xFFFF),0x0)) {
          Store(TBX,TB3); Store(Arg0,TE3); Return( And(TBX,0xFFFF) )
        }
        If(LEqual(And(TB4,0xFFFF),0x0)) {
          Store(TBX,TB4); Store(Arg0,TE4); Return( And(TBX,0xFFFF) )
        }
        If(LEqual(And(TB5,0xFFFF),0x0)) {
          Store(TBX,TB5); Store(Arg0,TE5); Return( And(TBX,0xFFFF) )
        }
        If(LEqual(And(TB6,0xFFFF),0x0)) {
          Store(TBX,TB6); Store(Arg0,TE6); Return( And(TBX,0xFFFF) )
        }
        If(LEqual(And(TB7,0xFFFF),0x0)) {
          Store(TBX,TB7); Store(Arg0,TE7); Return( And(TBX,0xFFFF) )
        }
        Sleep(5)
        Local0++
      }
  }

  // EC_SVC_MANAGEMENT 330c1273-fde5-4757-9819-5b6539037502
  Method(ASYC, 0x0, Serialized) {  
    If(LEqual(\_SB.FFA0.AVAL,One)) {
      Name(BUFF, Buffer(30){})
    
      CreateByteField(BUFF,0,STAT) // Out – Status for req/rsp 
      CreateByteField(BUFF,1,LENG) // In/Out – Bytes in req, updates bytes returned 
      CreateField(BUFF,16,128,UUID) // UUID of service 
      CreateByteField(BUFF,18,CMDD) // Command register
      CreateWordField(BUFF,19,BSQN) // Sequence Number
      CreateField(BUFF,208,32,FWSD) // Output Data
      // x0 -> STAT

      Store(20, LENG)
      Store(0x0, CMDD) // EC_ASYNC command
      Local0 = QTXB(BUFF,20)

      Store(Local0,BSQN) // Sequence packet to read from shared memory
      Store(ToUUID("330c1273-fde5-4757-9819-5b6539037502"), UUID)
      Store(Store(BUFF, \_SB_.FFA0.FFAC), BUFF)

      If(LEqual(STAT,0x0) ) // Check FF-A successful?
      {
        Return (RXDB(Local0))
      } else {
        Return(Zero)
      }
    } else {
      Return(Zero)
    }
  }

  // EC_SVC_MANAGEMENT 330c1273-fde5-4757-9819-5b6539037502
  Method(TFWS, 0x0, Serialized) {  
    If(LEqual(\_SB.FFA0.AVAL,One)) {
      Name(BUFF, Buffer(30){})
    
      CreateByteField(BUFF,0,STAT) // Out – Status for req/rsp 
      CreateByteField(BUFF,1,LENG) // In/Out – Bytes in req, updates bytes returned 
      CreateField(BUFF,16,128,UUID) // UUID of service 
      CreateByteField(BUFF,18,CMDD) // Command register
      
      CreateField(BUFF,208,32,FWSD) // Output Data
      // x0 -> STAT

      Store(20, LENG)
      Store(0x1, CMDD) // EC_CAP_GET_FW_STATE
      Store(ToUUID("330c1273-fde5-4757-9819-5b6539037502"), UUID)
      Store(Store(BUFF, \_SB_.FFA0.FFAC), BUFF)
      If(LEqual(STAT,0x0) ) // Check FF-A successful?
      {
        Return (FWSD)
      } else {
        Return(Zero)
      }
    } else {
      Return(Zero)
    }
  }

  // Call test API to send notification event
  Method(TNFY, 0x0, Serialized) {  
    If(LEqual(\_SB.FFA0.AVAL,One)) {
      Name(BUFF, Buffer(30){})
    
      CreateByteField(BUFF,1,LENG) // In/Out – Bytes in req, updates bytes returned 
      CreateField(BUFF,16,128,UUID) // UUID of service 
      CreateByteField(BUFF,18,CMDD) // Command register
  
      Store(20, LENG)
      Store(4, CMDD) // EC_CAP_TEST_NFY
      Store(ToUUID("330c1273-fde5-4757-9819-5b6539037502"), UUID)
      Store(Store(BUFF, \_SB_.FFA0.FFAC), BUFF)
    }
  }

} // Device (ECT0)


Device(\_SB_.FFA0) {
  Name(_HID, "MSFT000C")
  
  // Register notification events from FFA
  Method(_RNY, 0, Serialized) {
    Return( Package() {
      Package(0x2) {
        ToUUID("330c1273-fde5-4757-9819-5b6539037502"),
        Buffer() {0x1,0x0,0x2,0x0,0x3,0x0} // Register events 0x1, 0x2, 0x3
      }
    } )
  }   

  OperationRegion(AFFH, FFixedHw, 4, 144) 
  Field(AFFH, BufferAcc, NoLock, Preserve) { AccessAs(BufferAcc, 0x1), FFAC, 1152 }     

  Method(_NFY, 2, Serialized) {
    // Arg0 == UUID
    // Arg1 == Notify ID

    If(LEqual(ToUUID("330c1273-fde5-4757-9819-5b6539037502"),Arg0)) {
      Store(Arg1, \_SB.ECT0.NEVT)
      Notify(\_SB.ECT0, 0x20)
    }

  }

  // Other components check this to make sure FFA is available
  Method(AVAL, 0, Serialized) {
    Return(One)
  }

}

