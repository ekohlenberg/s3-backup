using System;
using System.Reflection;

namespace s3b
{
    public class ObjectInfo:Model
    {
        public ObjectInfo()
        {
        }

        static public ObjectInfo factory(string[] parts )
        {
            ObjectInfo o = new ObjectInfo();

            if (parts.Length != 4) return o;

            DateTime uploadDateTime;
            DateTime.TryParse(parts[0] + " " + parts[1], out uploadDateTime);
            
            int encryptedFileSize = 0;
            Int32.TryParse(parts[2], out encryptedFileSize);

            string encryptedFileName = parts[3];

            o.encrypted_file_name = encryptedFileName;
            o.encrypted_file_size = encryptedFileSize;
            o.upload_date_time = uploadDateTime;

            return o;

        }
        public string encrypted_file_name
        {
            get
            {
                return getPropValue(MethodBase.GetCurrentMethod().Name).ToString();
            }
            set
            {
                setPropValue(MethodBase.GetCurrentMethod().Name, value);
            }
        }


        public string encrypted_base_file_name
        {
            get
            {
                string filename = encrypted_file_name;

                filename = filename.Replace(".enc", "");
                filename = filename.Replace(".gz", "");
                filename = filename.Replace(".tar", "");

                return filename;
            }
        }
        public long encrypted_file_size
        {
            get
            {
                return Convert.ToInt64(getPropValue(MethodBase.GetCurrentMethod().Name));
            }
            set
            {
                setPropValue(MethodBase.GetCurrentMethod().Name, value);
            }
        }

        public DateTime upload_date_time
        {
            get
            {
                return Convert.ToDateTime(getPropValue(MethodBase.GetCurrentMethod().Name));
            }
            set
            {
                setPropValue(MethodBase.GetCurrentMethod().Name, value);
            }
        }


    }
}

